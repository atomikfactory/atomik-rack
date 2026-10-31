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
        [vec![pid(&c, CUTOFF)], vec![], vec![pid(&c, CUTOFF)], vec![]]
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
            controllers: Vec::new(),
            name: None,
            color: None,
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

// ------------------------------------------------------------------ a channel strip

#[test]
fn a_take_on_a_channel_fader_lane_writes_its_automation_track_in_one_undo_step() {
    // The full registry: the synth gets a console channel, whose fader gets an automation track.
    let mut c = AppCore::with_registry(
        RecordingSink::new(),
        atomik_render::default_registry(),
        atomik_model::empty_doc(),
        None,
    );
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key("a")),
    })
    .unwrap();
    let channel = c.doc().console.channels[0].channel.clone();
    c.apply_command(song(SongCommand::AddAutomationLane {
        device: channel.clone(),
        param: "fader".into(),
    }))
    .unwrap();
    c.sink_mut().take();
    c.handle_action(UiAction::ToggleLaneArm {
        device: channel.clone(),
        param: "fader".into(),
    });
    assert!(c.automation_armed(&channel, "fader"));
    record(&mut c);
    assert!(c.is_recording());
    let depth = |c: &mut AppCore<RecordingSink>| {
        let mut n = 0;
        while c.undo() {
            n += 1;
        }
        while c.redo() {}
        n
    };
    for (tick, value) in [(1000, 0.3), (2000, 0.6), (3000, 0.9)] {
        at(&mut c, tick, 0);
        c.apply_command(Command::SetParam {
            key: channel.clone(),
            param: "fader".into(),
            value,
            merge: Some(7),
        })
        .unwrap();
    }
    release(&mut c);
    let before = depth(&mut c);
    at(&mut c, 4000, 0);
    record(&mut c);
    assert!(!c.is_recording());
    let track = c.doc().song.track(&channel).unwrap();
    assert!(track.is_automation_only());
    let lane = &track.lanes[0];
    assert_eq!(lane.clips.len(), 1, "the take wrote one clip");
    let ClipContent::Automation(points) = &lane.clips[0].content else {
        panic!("automation");
    };
    assert!(points.len() >= 2 && (points.last().unwrap().value - 0.9).abs() < 1e-3);
    // The take is one undo step, on top of the knob moves.
    assert_eq!(depth(&mut c), before + 1);
}

/// How many undo steps the document has (undone and redone again).
fn undo_depth(c: &mut AppCore<RecordingSink>) -> usize {
    let mut n = 0;
    while c.undo() {
        n += 1;
    }
    while c.redo() {}
    n
}

// ------------------------------------------------------------------ a muted lane holds its value

/// A one-point step clip on lane `lane` of `a`: the lane gives `value` from `start` on.
fn step_clip(start: i64, length: i64, value: f32) -> atomik_model::Clip {
    atomik_model::Clip {
        id: atomik_model::ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Automation(vec![AutomationPoint {
            tick: 0,
            value,
            curve: atomik_model::Curve::Step,
        }]),
    }
}

fn add_step_clip(c: &mut AppCore<RecordingSink>, lane: usize, clip: atomik_model::Clip) {
    c.apply_command(song(SongCommand::AddClip {
        device: key("a"),
        lane,
        clip,
    }))
    .unwrap();
}

#[test]
fn muting_a_lane_holds_the_value_it_gave_at_the_playhead_in_one_undo_step() {
    let mut c = core();
    add_step_clip(&mut c, 1, step_clip(BAR, BAR, 4000.0));
    let before = c.doc().clone();
    c.transport_action(TransportAction::Locate(BAR + PPQ));
    let depth = undo_depth(&mut c);
    c.toggle_automation_lane_mute(&key("a"), CUTOFF);
    let doc = c.doc();
    assert!(doc.song.tracks[0].lanes[1].muted);
    assert_eq!(
        doc.param_value(&key("a"), CUTOFF, c.registry()),
        Some(4000.0),
        "the param keeps what the lane gave"
    );
    assert_eq!(undo_depth(&mut c), depth + 1, "one undo step");
    // Unmuting changes only the flag.
    c.toggle_automation_lane_mute(&key("a"), CUTOFF);
    assert!(!c.doc().song.tracks[0].lanes[1].muted);
    assert!(c.undo());
    assert!(c.undo());
    assert_eq!(c.doc(), &before);
}

#[test]
fn muting_a_lane_that_plays_nothing_changes_only_its_flag() {
    let mut c = core();
    let value = c.doc().param_value(&key("a"), RESO, c.registry());
    c.toggle_automation_lane_mute(&key("a"), RESO);
    assert!(c.doc().song.tracks[0].lanes[2].muted);
    assert_eq!(c.doc().param_value(&key("a"), RESO, c.registry()), value);
}

// ------------------------------------------------------------------ auto-created lanes

const DRIVE: &str = "filter.key_track";

/// A core with synths `a` and `b` and no lane at all (the start-up calls taken).
fn bare() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    c.sink_mut().take();
    c
}

fn set_on(
    c: &mut AppCore<RecordingSink>,
    device: &str,
    param: &str,
    value: f32,
    merge: Option<u64>,
) {
    c.apply_command(Command::SetParam {
        key: key(device),
        param: param.into(),
        value,
        merge,
    })
    .unwrap();
}

/// The automation lanes of `device`'s track as `(param, clip count)`.
fn lanes_of(c: &AppCore<RecordingSink>, device: &str) -> Vec<(String, usize)> {
    c.doc()
        .song
        .track(&key(device))
        .map(|t| {
            t.lanes
                .iter()
                .filter_map(|l| match &l.kind {
                    atomik_model::LaneKind::Automation(p) => Some((p.clone(), l.clips.len())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn recording_needs_an_armed_lane_or_auto_create_lanes() {
    let mut c = bare();
    record(&mut c);
    assert!(!c.is_recording());
    c.transport_action(TransportAction::ToggleAutoLanes);
    assert!(c.auto_create_lanes());
    record(&mut c);
    assert!(c.is_recording() && c.is_playing());
    // The switch is session state: nothing in the project.
    assert!(!atomik_model::save_project(c.doc()).contains("auto"));
}

#[test]
fn two_knobs_on_two_devices_make_two_lanes_with_clips_and_one_undo_removes_them() {
    let mut c = bare();
    c.set_auto_create_lanes(true);
    record(&mut c);
    c.sink_mut().take();
    at(&mut c, 1000, 0);
    set_on(&mut c, "a", CUTOFF, 1000.0, Some(7));
    // A lane that is still to be made holds nothing in the engine.
    assert!(holds(&c.sink_mut().take()).is_empty());
    at(&mut c, 2000, 0);
    set_on(&mut c, "a", CUTOFF, 5000.0, Some(7));
    release(&mut c);
    at(&mut c, 3000, 0);
    set_on(&mut c, "b", DRIVE, 0.5, Some(8));
    at(&mut c, 4000, 0);
    set_on(&mut c, "b", DRIVE, 0.9, Some(8));
    release(&mut c);
    // Nothing is written before the take ends.
    assert!(lanes_of(&c, "a").is_empty() && lanes_of(&c, "b").is_empty());
    let depth = undo_depth(&mut c);
    at(&mut c, 6000, 0);
    record(&mut c);
    assert!(!c.is_recording());
    assert_eq!(lanes_of(&c, "a"), [(CUTOFF.to_string(), 1)]);
    assert_eq!(lanes_of(&c, "b"), [(DRIVE.to_string(), 1)]);
    assert_eq!(
        undo_depth(&mut c),
        depth + 1,
        "lanes and clips are one undo step"
    );
    assert!(c.doc().validate().is_ok());
    assert!(c.undo());
    assert!(lanes_of(&c, "a").is_empty() && lanes_of(&c, "b").is_empty());
    assert!(
        c.doc().song.track(&key("b")).is_none(),
        "no empty track is left"
    );
    assert!(c.redo());
    assert_eq!(lanes_of(&c, "b"), [(DRIVE.to_string(), 1)]);
}

#[test]
fn a_parameter_with_a_lane_records_only_when_armed_even_with_auto_create_on() {
    let mut c = core();
    c.set_auto_create_lanes(true);
    arm(&mut c, CUTOFF);
    record(&mut c);
    at(&mut c, 1000, 0);
    set(&mut c, CUTOFF, 1000.0, Some(7));
    at(&mut c, 2000, 0);
    set(&mut c, CUTOFF, 6000.0, Some(7));
    release(&mut c);
    // The resonance lane exists but is not armed: it takes nothing.
    set(&mut c, RESO, 0.4, Some(9));
    release(&mut c);
    at(&mut c, 4000, 0);
    record(&mut c);
    assert_eq!(clips(&c, 1).len(), 1);
    assert!(clips(&c, 2).is_empty());
    assert_eq!(c.doc().song.track(&key("a")).unwrap().lanes.len(), 3);
}

#[test]
fn a_macro_driven_parameter_is_ignored_with_a_message() {
    let mut c = bare();
    assert!(c.assign_to_macro(&key("b"), "filter.resonance", 0, true));
    let bank = c.doc().banks().last().unwrap().key.clone();
    c.set_auto_create_lanes(true);
    record(&mut c);
    at(&mut c, 1000, 0);
    // The driven param refuses a direct edit; the engine-side change of a driven param that
    // reaches the take (a plugin window, say) is what the message is about.
    let ops = [SyncOp::ParamChanged {
        key: key("b"),
        param: "filter.resonance".into(),
        value: 0.3,
    }];
    c.capture_automation(&ops, Some(3));
    assert!(
        c.last_rejection().unwrap().contains("driven by a macro"),
        "{:?}",
        c.last_rejection()
    );
    // The macro's own knob is an ordinary param and gets its lane.
    c.apply_command(Command::SetParam {
        key: bank.clone(),
        param: "macro1".into(),
        value: 0.7,
        merge: Some(4),
    })
    .unwrap();
    at(&mut c, 2000, 0);
    release(&mut c);
    at(&mut c, 3000, 0);
    record(&mut c);
    assert!(lanes_of(&c, "b").is_empty(), "no lane for the driven param");
    assert_eq!(lanes_of(&c, &bank.0), [("macro1".to_string(), 1)]);
}

// ------------------------------------------------------------------ override

#[test]
fn a_control_moved_by_hand_during_playback_holds_its_lane_until_stop() {
    let mut c = core();
    add_step_clip(&mut c, 1, step_clip(0, 4 * BAR, 1000.0));
    c.transport_action(TransportAction::Play);
    at(&mut c, 500, 0);
    c.sink_mut().take();
    let cutoff = pid(&c, CUTOFF);
    // A change without a gesture id (a text entry, a click) does not override.
    set(&mut c, CUTOFF, 2000.0, None);
    assert_eq!(c.overridden_lanes(), 0);
    set(&mut c, CUTOFF, 3000.0, Some(5));
    release(&mut c);
    assert_eq!(c.overridden_lanes(), 1);
    assert_eq!(holds(&c.sink_mut().take()), [vec![cutoff]]);
    // The control shows the user's value, not the lane's.
    assert!(
        !c.automation_shown()
            .contains_key(&(key("a"), CUTOFF.to_string()))
    );
    // The lamp clears it; moving again holds again; Stop clears it too.
    c.transport_action(TransportAction::ClearOverride);
    assert_eq!(c.overridden_lanes(), 0);
    assert_eq!(holds(&c.sink_mut().take()), [Vec::new()]);
    set(&mut c, CUTOFF, 3500.0, Some(6));
    assert_eq!(c.overridden_lanes(), 1);
    c.transport_action(TransportAction::Stop);
    assert_eq!(c.overridden_lanes(), 0);
    // A parameter without a lane is never overridden.
    c.transport_action(TransportAction::Play);
    set(&mut c, DRIVE, 0.5, Some(7));
    assert_eq!(c.overridden_lanes(), 0);
}

#[test]
fn an_override_is_dropped_when_its_lane_is_removed_and_on_a_locate() {
    let mut c = core();
    add_step_clip(&mut c, 2, step_clip(0, BAR, 0.2));
    c.transport_action(TransportAction::Play);
    at(&mut c, 100, 0);
    set(&mut c, RESO, 0.6, Some(5));
    assert_eq!(c.overridden_lanes(), 1);
    c.delete_automation_lane(&key("a"), RESO);
    c.feed_automation_values();
    assert_eq!(c.overridden_lanes(), 0, "the lane is gone");
    set(&mut c, CUTOFF, 0.6, Some(5));
    c.transport_action(TransportAction::Locate(0));
    assert_eq!(c.overridden_lanes(), 0);
}

// ---- tempo pass -----------------------------------------------------------------------

fn tempo_drag(c: &mut AppCore<RecordingSink>, bpm: f64, merge: u64) {
    c.apply_command(song(SongCommand::DragTempoMap {
        map: vec![atomik_model::TempoEvent::step(0, bpm)],
        merge,
    }))
    .unwrap();
}

fn tempo_map(c: &AppCore<impl EngineSink>) -> Vec<(i64, f64, bool)> {
    c.doc()
        .song
        .tempo
        .iter()
        .map(|e| (e.tick, e.bpm, e.ramp))
        .collect()
}

#[test]
fn a_tempo_pass_records_the_drag_as_a_ramp_and_one_undo_restores_the_old_map() {
    let mut c = core();
    c.apply_command(song(SongCommand::SetTempoMap(vec![
        atomik_model::TempoEvent::step(0, 100.0),
    ])))
    .unwrap();
    c.toggle_tempo_arm();
    assert!(c.tempo_armed());
    at(&mut c, 0, 0);
    record(&mut c);
    // Eight bars of playback while the tempo is dragged from 100 to 130.
    for step in 1..=32_i64 {
        at(&mut c, step * BAR / 4, 0);
        tempo_drag(&mut c, 100.0 + 30.0 * step as f64 / 32.0, 7);
    }
    release(&mut c);
    at(&mut c, 8 * BAR, 0);
    record(&mut c);
    let map = tempo_map(&c);
    assert!(map.len() >= 2, "a ramp-like series: {map:?}");
    assert!(map.len() < 33, "thinned: {map:?}");
    assert_eq!(map[0].0, 0);
    assert!(
        map[..map.len() - 1].iter().all(|e| e.2),
        "ramps into the last: {map:?}"
    );
    assert!(
        (map.last().unwrap().1 - 130.0).abs() < 0.01,
        "ends at 130: {map:?}"
    );
    assert!(c.undo());
    assert_eq!(
        tempo_map(&c),
        [(0, 100.0, false)],
        "one undo restores the old map"
    );
}

#[test]
fn an_unarmed_take_leaves_a_tempo_drag_as_the_user_made_it() {
    let mut c = core();
    at(&mut c, 0, 0);
    record(&mut c);
    // Nothing is armed: the take refuses to start, the drag is a plain edit.
    for step in 1..=8_i64 {
        at(&mut c, step * BAR / 4, 0);
        tempo_drag(&mut c, 100.0 + 30.0 * step as f64 / 8.0, 7);
    }
    release(&mut c);
    assert_eq!(tempo_map(&c), [(0, 130.0, false)]);
}

#[test]
fn a_tempo_take_that_never_moves_the_tempo_changes_nothing() {
    let mut c = core();
    let before = tempo_map(&c);
    c.toggle_tempo_arm();
    at(&mut c, 0, 0);
    record(&mut c);
    at(&mut c, 2 * BAR, 0);
    record(&mut c);
    assert_eq!(tempo_map(&c), before);
}
