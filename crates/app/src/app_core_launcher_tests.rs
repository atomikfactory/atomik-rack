//! The clip launcher in the app core: launches and stops reach the engine as transport commands
//! with the right quantize, a launch while stopped plays, the engine's published state reaches
//! the view, and Record Launches writes what was launched into the arrangement as one undo
//! step.

use super::tests::rack_only_core;
use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_engine::{LaunchCommand, LaunchEvent, LaunchGrid, LaunchState, TransportCommand};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, LaunchQuantize, LauncherCommand, Note,
    SongCommand,
};
use atomik_ui::{LauncherAction, TransportAction};

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

fn launcher(c: LauncherCommand) -> Command {
    Command::Song(SongCommand::Launcher(c))
}

/// A core with synths `a` (engine id 1) and `b` (id 2) on tracks, and a one-bar clip in slot 0
/// of both, a two-bar clip in slot 1 of `a`.
fn with_slots() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key(k)),
        })
        .unwrap();
        c.apply_command(song(SongCommand::AddTrack { device: key(k) }))
            .unwrap();
    }
    for (k, row, length) in [("a", 0, BAR), ("b", 0, BAR), ("a", 1, 2 * BAR)] {
        c.apply_command(launcher(LauncherCommand::NewClip {
            device: key(k),
            row,
            length,
        }))
        .unwrap();
    }
    c.sink_mut().take();
    c
}

fn launches(calls: &[SinkCall]) -> Vec<LaunchCommand> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Transport(TransportCommand::Launch(l)) => Some(*l),
            _ => None,
        })
        .collect()
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

fn id(n: u32) -> atomik_engine::DeviceInstanceId {
    atomik_engine::DeviceInstanceId(n)
}

#[test]
fn a_launch_goes_to_the_engine_with_the_song_quantize_and_starts_a_stopped_transport() {
    let mut c = with_slots();
    c.launch_slot(&key("a"), 1);
    let calls = c.sink_mut().take();
    assert_eq!(
        launches(&calls),
        vec![LaunchCommand::Launch {
            device: id(1),
            slot: 1,
            grid: LaunchGrid::Bars(1)
        }]
    );
    // The transport was stopped: it plays now (the launch waits for its boundary).
    assert!(transports(&calls).contains(&TransportCommand::Play));
    // Already playing: no second Play.
    c.launch_slot(&key("b"), 0);
    let calls = c.sink_mut().take();
    assert!(!transports(&calls).contains(&TransportCommand::Play));
}

#[test]
fn a_slot_quantize_and_the_song_quantize_decide_the_grid() {
    let mut c = with_slots();
    c.apply_command(launcher(LauncherCommand::SetQuantize(
        LaunchQuantize::Quarter,
    )))
    .unwrap();
    c.launch_slot(&key("a"), 0);
    assert_eq!(
        launches(&c.sink_mut().take()),
        vec![LaunchCommand::Launch {
            device: id(1),
            slot: 0,
            grid: LaunchGrid::Ticks(PPQ)
        }]
    );
    c.apply_command(launcher(LauncherCommand::EditSlot {
        device: key("a"),
        row: 0,
        name: atomik_model::MetaChange::Keep,
        color: atomik_model::MetaChange::Keep,
        length: None,
        quantize: Some(Some(LaunchQuantize::FourBars)),
    }))
    .unwrap();
    c.sink_mut().take();
    c.launch_slot(&key("a"), 0);
    assert_eq!(
        launches(&c.sink_mut().take()),
        vec![LaunchCommand::Launch {
            device: id(1),
            slot: 0,
            grid: LaunchGrid::Bars(4)
        }]
    );
    // An empty slot launches nothing.
    c.launch_slot(&key("b"), 5);
    assert!(launches(&c.sink_mut().take()).is_empty());
}

#[test]
fn a_scene_launches_every_track_and_stops_the_empty_ones_that_play() {
    let mut c = with_slots();
    c.launch_scene(0);
    let calls = c.sink_mut().take();
    assert_eq!(launches(&calls).len(), 2);
    // Scene 2 has a clip on `a` only; `b` plays slot 0 and stops (scenes stop empty tracks).
    c.sink_mut().launch_state = vec![
        LaunchState {
            device: id(1),
            playing: Some(0),
            queued: None,
            queued_stop: false,
            phase: 0.1,
        },
        LaunchState {
            device: id(2),
            playing: Some(0),
            queued: None,
            queued_stop: false,
            phase: 0.1,
        },
    ];
    c.frame();
    c.sink_mut().take();
    c.launch_scene(1);
    let l = launches(&c.sink_mut().take());
    assert!(l.contains(&LaunchCommand::Launch {
        device: id(1),
        slot: 1,
        grid: LaunchGrid::Bars(1)
    }));
    assert!(l.contains(&LaunchCommand::Stop {
        device: id(2),
        grid: LaunchGrid::Bars(1)
    }));
    // With the setting off, the empty track plays on.
    c.apply_command(launcher(LauncherCommand::SetStopEmpty(false)))
        .unwrap();
    c.sink_mut().take();
    c.launch_scene(1);
    let l = launches(&c.sink_mut().take());
    assert_eq!(l.len(), 1);
}

#[test]
fn back_to_arrangement_is_immediate_and_stop_all_waits_for_the_boundary() {
    let mut c = with_slots();
    c.launcher_back();
    assert_eq!(
        launches(&c.sink_mut().take()),
        vec![LaunchCommand::StopAll {
            grid: LaunchGrid::Ticks(1)
        }]
    );
    c.stop_all_slots(false);
    assert_eq!(
        launches(&c.sink_mut().take()),
        vec![LaunchCommand::StopAll {
            grid: LaunchGrid::Bars(1)
        }]
    );
    c.stop_launch_track(&key("b"));
    assert_eq!(
        launches(&c.sink_mut().take()),
        vec![LaunchCommand::Stop {
            device: id(2),
            grid: LaunchGrid::Bars(1)
        }]
    );
}

#[test]
fn the_engine_state_reaches_the_view_by_track() {
    let mut c = with_slots();
    c.set_launcher_visible(true);
    c.sink_mut().launch_state = vec![LaunchState {
        device: id(2),
        playing: Some(0),
        queued: Some(1),
        queued_stop: false,
        phase: 0.25,
    }];
    // The launch list is in the order of the tracks: a is first. The state list follows it.
    c.sink_mut().launch_state.insert(
        0,
        LaunchState {
            device: id(1),
            playing: None,
            queued: None,
            queued_stop: true,
            phase: 0.0,
        },
    );
    c.frame();
    let feed = c.launcher().feed();
    assert_eq!(feed.tracks[&key("b")].playing, Some(0));
    assert_eq!(feed.tracks[&key("b")].queued, Some(1));
    assert!(feed.tracks[&key("a")].queued_stop);
}

#[test]
fn a_new_clip_made_in_the_view_opens_in_the_piano_roll_and_edits_stay_in_the_slot() {
    let mut c = with_slots();
    c.launcher_action(LauncherAction::NewClip {
        device: key("a"),
        row: 4,
    });
    let slot = c
        .doc()
        .song
        .launcher
        .slot(&key("a"), 4)
        .expect("the clip exists")
        .clone();
    assert_eq!(slot.clip.length, BAR);
    assert!(c.piano_roll_open());
    assert_eq!(c.piano_roll_clip(), Some(slot.clip.id));
    // A note added by the roll's command lands in the slot, not in the arrangement.
    c.apply_command(song(SongCommand::SetClipContent {
        id: slot.clip.id,
        content: ClipContent::Notes(vec![Note::new(0, PPQ, 60, 0.8)].into()),
        merge: None,
    }))
    .unwrap();
    let after = c.doc().song.launcher.slot(&key("a"), 4).unwrap();
    assert!(matches!(&after.clip.content, ClipContent::Notes(n) if n.len() == 1));
    assert!(
        c.doc().song.track(&key("a")).unwrap().lanes[0]
            .clips
            .is_empty()
    );
}

fn note_slot(c: &mut AppCore<RecordingSink>, track: &str, row: usize) -> ClipId {
    let id = c
        .doc()
        .song
        .launcher
        .slot(&key(track), row)
        .unwrap()
        .clip
        .id;
    c.apply_command(song(SongCommand::SetClipContent {
        id,
        content: ClipContent::Notes(vec![Note::new(0, PPQ, 60, 0.8)].into()),
        merge: None,
    }))
    .unwrap();
    id
}

fn event(device: u32, slot: Option<u16>, tick: i64) -> LaunchEvent {
    LaunchEvent {
        device: id(device),
        slot,
        tick,
    }
}

fn lane_clips(c: &AppCore<RecordingSink>, track: &str) -> Vec<Clip> {
    c.doc().song.track(&key(track)).unwrap().lanes[0]
        .clips
        .clone()
}

#[test]
fn recording_launches_writes_the_launched_clips_as_one_undo_step() {
    let mut c = with_slots();
    note_slot(&mut c, "a", 0);
    note_slot(&mut c, "a", 1);
    note_slot(&mut c, "b", 0);
    c.set_record_launches(true);
    // No armed track: the record button still starts a launch pass, and plays.
    c.transport_action(TransportAction::ToggleRecord);
    assert!(c.recording_launches());
    assert!(
        transports(&c.sink_mut().take()).contains(&TransportCommand::Play),
        "recording launches plays"
    );
    // a: slot 0 from bar 2, slot 1 (two bars) from bar 4 to bar 6; b: slot 0 from bar 2 to the
    // end of the pass at bar 7.
    let events = [
        event(1, Some(0), BAR),
        event(2, Some(0), BAR),
        event(1, Some(1), 3 * BAR),
        event(1, None, 5 * BAR),
    ];
    c.sink_mut().launch_events.extend(events);
    let depth = c.document().undo_depth();
    c.sink_mut().state.playing = true;
    c.sink_mut().state.tick = 6 * BAR;
    c.transport_action(TransportAction::Stop);
    assert!(!c.recording_launches());
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    let a = lane_clips(&c, "a");
    let spans: Vec<(i64, i64, Option<i64>)> = a
        .iter()
        .map(|k| (k.start, k.length, k.loop_length))
        .collect();
    // Slot 0 played bars 2 and 3 (two cycles of a one-bar loop), slot 1 played one cycle of two
    // bars exactly.
    assert_eq!(
        spans,
        vec![(BAR, 2 * BAR, Some(BAR)), (3 * BAR, 2 * BAR, None)]
    );
    let b = lane_clips(&c, "b");
    assert_eq!(
        b.iter()
            .map(|k| (k.start, k.length, k.loop_length))
            .collect::<Vec<_>>(),
        vec![(BAR, 5 * BAR, Some(BAR))]
    );
    // The clips carry the slot's notes.
    assert!(matches!(&a[0].content, ClipContent::Notes(n) if n.len() == 1));
    // One undo takes the whole pass away.
    c.undo();
    assert!(lane_clips(&c, "a").is_empty() && lane_clips(&c, "b").is_empty());
}

#[test]
fn a_pass_with_no_launches_writes_nothing() {
    let mut c = with_slots();
    c.set_record_launches(true);
    c.transport_action(TransportAction::ToggleRecord);
    let depth = c.document().undo_depth();
    c.transport_action(TransportAction::ToggleRecord);
    assert!(!c.recording_launches());
    assert_eq!(c.document().undo_depth(), depth);
}

#[test]
fn launches_are_not_recorded_while_record_launches_is_off() {
    let mut c = with_slots();
    c.sink_mut().launch_events.push_back(event(1, Some(0), BAR));
    c.frame();
    c.transport_action(TransportAction::Stop);
    assert!(lane_clips(&c, "a").is_empty());
}
