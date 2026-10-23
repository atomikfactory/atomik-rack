//! Automation recording through the core: a scripted transport, the recording sink and the
//! document. The write modes, the hold, loop passes, thinning and the one undo step per take.

use super::tests::rack_only_core;
use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_engine::TransportState;
use atomik_model::automation_rec::WriteMode;
use atomik_model::{AutomationPoint, ClipContent, Command, DeviceKey, SongCommand};
use atomik_ui::{TransportAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";
const CUTOFF: &str = "filter.cutoff";
const RESO: &str = "filter.resonance";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

/// A core with synth `a` (engine id 1), a track and lanes for the cutoff and the resonance
/// (lane indexes 1 and 2), the start-up calls taken.
fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    for cmd in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        },
        song(SongCommand::AddTrack { device: key("a") }),
        song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: CUTOFF.into(),
        }),
        song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: RESO.into(),
        }),
    ] {
        c.apply_command(cmd).unwrap();
    }
    c.sink_mut().take();
    c
}

fn set_mode(c: &mut AppCore<RecordingSink>, param: &str, mode: WriteMode) {
    c.apply_command(song(SongCommand::SetLaneWriteMode {
        device: key("a"),
        param: param.into(),
        mode,
    }))
    .unwrap();
}

fn arm(c: &mut AppCore<RecordingSink>, param: &str) {
    c.handle_action(UiAction::ToggleLaneArm {
        device: key("a"),
        param: param.into(),
    });
}

/// The engine is at song tick `tick` (loop lap `lap`, no loop range unless `looped`).
fn at(c: &mut AppCore<RecordingSink>, tick: i64, lap: u64) {
    c.sink_mut().state = TransportState {
        playing: true,
        tick,
        engine_frame: 1000 + tick as u64,
        loop_count: lap,
        loop_start: 0,
        loop_end: BAR,
        loop_enabled: lap > 0,
        ..TransportState::default()
    };
}

fn set(c: &mut AppCore<RecordingSink>, param: &str, value: f32, merge: Option<u64>) {
    c.apply_command(Command::SetParam {
        key: key("a"),
        param: param.into(),
        value,
        merge,
    })
    .unwrap();
}

fn record(c: &mut AppCore<RecordingSink>) {
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
}

fn release(c: &mut AppCore<RecordingSink>) {
    c.handle_action(UiAction::EndMerge);
}

/// The clips of lane `lane` as `(start, length, points)`.
/// A recorded clip: start, length and its `(tick, value)` points.
type Written = (i64, i64, Vec<(i64, f32)>);

fn clips(c: &AppCore<impl EngineSink>, lane: usize) -> Vec<Written> {
    c.doc().song.track(&key("a")).unwrap().lanes[lane]
        .clips
        .iter()
        .map(|cl| {
            let ClipContent::Automation(p) = &cl.content else {
                panic!("not automation");
            };
            (
                cl.start,
                cl.length,
                p.iter()
                    .map(|p: &AutomationPoint| (p.tick, p.value))
                    .collect(),
            )
        })
        .collect()
}

fn holds(calls: &[SinkCall]) -> Vec<Vec<(DeviceInstanceId, ParamId)>> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Hold(h) => Some(h.clone()),
            _ => None,
        })
        .collect()
}

fn pid(c: &AppCore<RecordingSink>, param: &str) -> (DeviceInstanceId, ParamId) {
    let p = c
        .registry()
        .get(SYNTH)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id;
    (DeviceInstanceId(1), p)
}

// ------------------------------------------------------------------ arming

#[test]
fn a_lane_is_armed_and_disarmed_and_only_a_real_lane() {
    let mut c = core();
    assert!(!c.automation_armed(&key("a"), CUTOFF));
    arm(&mut c, CUTOFF);
    assert!(c.automation_armed(&key("a"), CUTOFF));
    assert!(!c.automation_armed(&key("a"), RESO));
    arm(&mut c, CUTOFF);
    assert!(!c.automation_armed(&key("a"), CUTOFF));
    arm(&mut c, "filter.drive");
    assert!(c.armed_automation_lanes().is_empty());
    assert!(c.last_rejection().unwrap().contains("no automation lane"));
}

#[test]
fn arming_is_not_an_undo_step_and_is_not_saved() {
    let mut c = core();
    let before = c.doc().clone();
    arm(&mut c, CUTOFF);
    assert_eq!(c.doc(), &before);
    let text = atomik_model::save_project(c.doc());
    assert!(!text.contains("armed_lanes"));
}

#[test]
fn recording_starts_with_only_a_lane_armed_and_needs_one_of_the_two() {
    let mut c = core();
    record(&mut c);
    assert!(!c.is_recording());
    assert!(c.last_rejection().unwrap().contains("arm"));
    arm(&mut c, CUTOFF);
    record(&mut c);
    assert!(c.is_recording() && c.is_playing());
    // A take that writes no notes does not arm any device for MIDI.
    assert!(c.sink_mut().take().contains(&SinkCall::Recording(vec![])));
}

#[test]
fn an_armed_lane_of_a_removed_track_is_dropped() {
    let mut c = core();
    arm(&mut c, CUTOFF);
    c.apply_command(song(SongCommand::RemoveTrack { device: key("a") }))
        .unwrap();
    record(&mut c);
    assert!(!c.is_recording());
    assert!(c.armed_automation_lanes().is_empty());
}

// ------------------------------------------------------------------ the write modes

#[test]
fn touch_writes_the_held_stretch_holds_the_lane_meanwhile_and_is_one_undo_step() {
    let mut c = core();
    arm(&mut c, CUTOFF);
    record(&mut c);
    c.sink_mut().take();
    let cutoff = pid(&c, CUTOFF);
    at(&mut c, 1000, 0);
    set(&mut c, CUTOFF, 1000.0, Some(7));
    assert_eq!(
        holds(&c.sink_mut().take()),
        [vec![cutoff]],
        "held from the touch"
    );
    at(&mut c, 2000, 0);
    set(&mut c, CUTOFF, 5000.0, Some(7));
    at(&mut c, 3000, 0);
    set(&mut c, CUTOFF, 9000.0, Some(7));
    assert!(
        holds(&c.sink_mut().take()).is_empty(),
        "the hold only changes when the set does"
    );
    at(&mut c, 4000, 0);
    release(&mut c);
    assert_eq!(
        holds(&c.sink_mut().take()),
        [vec![]],
        "released with the control"
    );
    at(&mut c, 8000, 0);
    record(&mut c);
    assert!(!c.is_recording());
    // The three moves are on a straight line: thinning keeps the ends.
    assert_eq!(
        clips(&c, 1),
        [(1000, 3000, vec![(0, 1000.0), (2000, 9000.0)])]
    );
    assert!(clips(&c, 2).is_empty());
    // One undo step for the whole take (the knob drag is its own step before it).
    let steps = c.doc().song.track(&key("a")).unwrap().lanes[1].clips.len();
    assert_eq!(steps, 1);
    assert!(c.undo());
    assert!(clips(&c, 1).is_empty(), "undo removes the take");
}

#[test]
fn touch_without_a_touch_writes_nothing() {
    let mut c = core();
    arm(&mut c, CUTOFF);
    record(&mut c);
    at(&mut c, 5000, 0);
    record(&mut c);
    assert!(clips(&c, 1).is_empty());
}

#[test]
fn two_touches_make_two_clips_and_the_lane_plays_between_them() {
    let mut c = core();
    arm(&mut c, CUTOFF);
    record(&mut c);
    c.sink_mut().take();
    for (from, to, v) in [(1000, 2000, 3000.0), (6000, 7000, 12000.0)] {
        at(&mut c, from, 0);
        set(&mut c, CUTOFF, v, Some(9));
        at(&mut c, to, 0);
        release(&mut c);
    }
    at(&mut c, 9000, 0);
    record(&mut c);
    assert_eq!(
        clips(&c, 1),
        [
            (1000, 1000, vec![(0, 3000.0)]),
            (6000, 1000, vec![(0, 12000.0)])
        ]
    );
    assert_eq!(
        holds(&c.sink_mut().take()),
        [
            vec![pid(&c, CUTOFF)],
            vec![],
            vec![pid(&c, CUTOFF)],
            vec![],
            vec![]
        ]
    );
}

#[test]
fn a_move_without_a_drag_is_a_touch_of_an_instant() {
    let mut c = core();
    arm(&mut c, CUTOFF);
    record(&mut c);
    c.sink_mut().take();
    at(&mut c, 1500, 0);
    set(&mut c, CUTOFF, 6000.0, None);
    assert!(
        holds(&c.sink_mut().take()).is_empty(),
        "begun and released in one step: the lane is never held"
    );
    at(&mut c, 4000, 0);
    record(&mut c);
    assert_eq!(clips(&c, 1), [(1500, 1, vec![(0, 6000.0)])]);
}

#[test]
fn latch_writes_from_the_first_touch_to_the_stop_whatever_the_release() {
    let mut c = core();
    set_mode(&mut c, CUTOFF, WriteMode::Latch);
    arm(&mut c, CUTOFF);
    record(&mut c);
    c.sink_mut().take();
    at(&mut c, 1000, 0);
    set(&mut c, CUTOFF, 2000.0, Some(3));
    at(&mut c, 2000, 0);
    release(&mut c);
    at(&mut c, 6000, 0);
    record(&mut c);
    assert_eq!(clips(&c, 1), [(1000, 5000, vec![(0, 2000.0)])]);
    assert_eq!(
        holds(&c.sink_mut().take()),
        [vec![pid(&c, CUTOFF)], vec![]],
        "held from the touch to the stop"
    );
}

#[test]
fn overwrite_replaces_the_whole_pass_from_the_value_it_started_with() {
    let mut c = core();
    // An old clip the take hides.
    c.apply_command(song(SongCommand::AddClip {
        device: key("a"),
        lane: 1,
        clip: atomik_model::Clip {
            id: atomik_model::ClipId(0),
            start: 0,
            length: BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            scale: None,
            content: ClipContent::Automation(vec![AutomationPoint {
                tick: 0,
                value: 500.0,
                curve: atomik_model::Curve::Linear,
            }]),
        },
    }))
    .unwrap();
    set_mode(&mut c, CUTOFF, WriteMode::Overwrite);
    let start_value = c
        .doc()
        .param_value(&key("a"), CUTOFF, c.registry())
        .unwrap();
    arm(&mut c, CUTOFF);
    c.sink_mut().take();
    record(&mut c);
    assert_eq!(
        holds(&c.sink_mut().take()),
        [vec![pid(&c, CUTOFF)]],
        "held from the start"
    );
    at(&mut c, 2000, 0);
    set(&mut c, CUTOFF, 15000.0, None);
    at(&mut c, 5000, 0);
    record(&mut c);
    let lane = clips(&c, 1);
    assert_eq!(lane.len(), 2);
    assert_eq!(
        lane[1],
        (0, 5000, vec![(0, start_value), (2000, 15000.0)]),
        "the new clip lies on the old one"
    );
}

#[test]
fn a_loop_wrap_ends_the_pass_and_starts_the_next_at_the_loop_start() {
    let mut c = core();
    set_mode(&mut c, CUTOFF, WriteMode::Overwrite);
    arm(&mut c, CUTOFF);
    record(&mut c);
    at(&mut c, 1000, 0);
    set(&mut c, CUTOFF, 4000.0, None);
    // The engine wraps at the loop end and plays on from the loop start.
    at(&mut c, 600, 1);
    c.frame();
    at(&mut c, 900, 1);
    set(&mut c, CUTOFF, 8000.0, None);
    at(&mut c, 1200, 1);
    record(&mut c);
    let lane = clips(&c, 1);
    assert_eq!(lane.len(), 2, "one clip per pass: {lane:?}");
    assert_eq!(lane[0].0..lane[0].0 + lane[0].1, 0..BAR);
    assert_eq!(lane[0].2.last().unwrap().1, 4000.0);
    assert_eq!((lane[1].0, lane[1].1), (0, 1200));
    assert_eq!(lane[1].2, [(0, 4000.0), (900, 8000.0)]);
}

// ------------------------------------------------------------------ what is captured

#[test]
fn only_armed_lanes_and_only_commands_while_a_take_runs_are_captured() {
    let mut c = core();
    set_mode(&mut c, CUTOFF, WriteMode::Latch);
    set_mode(&mut c, RESO, WriteMode::Latch);
    arm(&mut c, CUTOFF);
    // Before the take: nothing.
    at(&mut c, 500, 0);
    set(&mut c, CUTOFF, 111.0, None);
    record(&mut c);
    at(&mut c, 1000, 0);
    set(&mut c, RESO, 0.7, Some(5));
    set(&mut c, CUTOFF, 3000.0, Some(6));
    at(&mut c, 1500, 0);
    // Undo and redo are not captured.
    assert!(c.undo());
    assert!(c.redo());
    at(&mut c, 2000, 0);
    record(&mut c);
    assert!(clips(&c, 2).is_empty(), "an unarmed lane is not written");
    assert_eq!(clips(&c, 1), [(1000, 1000, vec![(0, 3000.0)])]);
}

#[test]
fn several_lanes_in_one_take_are_one_undo_step() {
    let mut c = core();
    for p in [CUTOFF, RESO] {
        set_mode(&mut c, p, WriteMode::Overwrite);
        arm(&mut c, p);
    }
    record(&mut c);
    at(&mut c, 1000, 0);
    set(&mut c, CUTOFF, 2000.0, None);
    set(&mut c, RESO, 0.5, None);
    at(&mut c, 3000, 0);
    record(&mut c);
    assert_eq!(clips(&c, 1).len(), 1);
    assert_eq!(clips(&c, 2).len(), 1);
    assert!(c.undo());
    assert!(clips(&c, 1).is_empty() && clips(&c, 2).is_empty());
    assert!(c.redo());
    assert_eq!(clips(&c, 1).len() + clips(&c, 2).len(), 2);
}

#[test]
fn recorded_points_are_thinned_by_the_tolerance_and_zero_keeps_them_all() {
    let moves = [
        (1000, 1000.0),
        (1100, 2000.0),
        (1200, 3000.0),
        (1300, 4000.0),
    ];
    for (tolerance, want) in [(0.005, 2), (0.0, 4)] {
        let mut c = core();
        c.set_automation_thinning(tolerance);
        set_mode(&mut c, CUTOFF, WriteMode::Latch);
        arm(&mut c, CUTOFF);
        record(&mut c);
        for (t, v) in moves {
            at(&mut c, t, 0);
            set(&mut c, CUTOFF, v, Some(2));
        }
        at(&mut c, 2000, 0);
        record(&mut c);
        assert_eq!(clips(&c, 1)[0].2.len(), want, "tolerance {tolerance}");
    }
}

#[test]
fn the_thinning_is_clamped_and_never_not_a_number() {
    let mut c = core();
    c.set_automation_thinning(5.0);
    assert!((c.automation_thinning() - 0.25).abs() < f32::EPSILON);
    c.set_automation_thinning(-1.0);
    assert_eq!(c.automation_thinning(), 0.0);
    c.set_automation_thinning(f32::NAN);
    assert!(c.automation_thinning() > 0.0);
}

#[test]
fn an_engine_restart_cancels_the_take_without_a_clip_and_releases_the_hold() {
    let mut c = core();
    set_mode(&mut c, CUTOFF, WriteMode::Overwrite);
    arm(&mut c, CUTOFF);
    record(&mut c);
    c.sink_mut().take();
    c.rebuild_engine();
    assert!(!c.is_recording());
    assert!(holds(&c.sink_mut().take()).contains(&vec![]));
    assert!(clips(&c, 1).is_empty());
}

#[test]
fn a_locate_ends_the_take_and_writes_it() {
    let mut c = core();
    set_mode(&mut c, CUTOFF, WriteMode::Latch);
    arm(&mut c, CUTOFF);
    record(&mut c);
    at(&mut c, 1000, 0);
    set(&mut c, CUTOFF, 2500.0, Some(1));
    at(&mut c, 2000, 0);
    c.handle_action(UiAction::Transport(TransportAction::Locate(BAR)));
    assert!(!c.is_recording());
    assert_eq!(clips(&c, 1), [(1000, 1000, vec![(0, 2500.0)])]);
}

#[test]
fn the_arrangement_is_told_which_lanes_are_armed_and_that_a_take_runs() {
    let mut c = core();
    arm(&mut c, CUTOFF);
    assert!(c.automation_armed(&key("a"), CUTOFF));
    record(&mut c);
    assert!(c.automation_writing(&key("a"), CUTOFF));
    assert!(!c.automation_writing(&key("a"), RESO));
    assert!(c.transport_bar().state().recording);
    at(&mut c, 100, 0);
    record(&mut c);
    assert!(!c.automation_writing(&key("a"), CUTOFF));
}

// ------------------------------------------------------------------ with a real engine

/// The whole path on an offline engine rendered by hand: the transport really runs, the lane is
/// held by the real engine, and the clip lands where the transport was when the knob moved.
#[test]
fn a_take_on_a_real_engine_lands_at_the_ticks_the_transport_played() {
    use crate::offline_sink::OfflineSink;
    let mut c = AppCore::with_doc(OfflineSink::new(), atomik_model::empty_doc(), None);
    for cmd in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        },
        song(SongCommand::AddTrack { device: key("a") }),
        song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: CUTOFF.into(),
        }),
        song(SongCommand::SetLaneWriteMode {
            device: key("a"),
            param: CUTOFF.into(),
            mode: WriteMode::Latch,
        }),
    ] {
        c.apply_command(cmd).unwrap();
    }
    let pump = |c: &mut AppCore<OfflineSink>, frames: usize| {
        let mut done = 0;
        while done < frames {
            let n = 480.min(frames - done);
            c.sink_mut().render(n);
            c.frame();
            done += n;
        }
    };
    c.handle_action(UiAction::ToggleLaneArm {
        device: key("a"),
        param: CUTOFF.into(),
    });
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    assert!(c.is_recording());
    // Half a second at 120 BPM is one beat.
    pump(&mut c, 24_000);
    c.apply_command(Command::SetParam {
        key: key("a"),
        param: CUTOFF.into(),
        value: 3000.0,
        merge: Some(1),
    })
    .unwrap();
    pump(&mut c, 24_000);
    c.handle_action(UiAction::EndMerge);
    pump(&mut c, 24_000);
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    assert!(!c.is_recording());
    let lane = clips(&c, 1);
    assert_eq!(lane.len(), 1, "{lane:?}");
    let (start, length, points) = &lane[0];
    // The move happened after one beat (PPQ ticks), and a latch runs to the stop (three beats).
    assert!(
        (start - PPQ).abs() <= PPQ / 8,
        "starts about one beat in: {start}"
    );
    assert!(
        (start + length - 3 * PPQ).abs() <= PPQ / 4,
        "ends about three beats in: {}",
        start + length
    );
    assert_eq!(points, &[(0, 3000.0)]);
}
