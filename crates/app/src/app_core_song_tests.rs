//! `AppCore` tests with the recording sink: song sync, transport mapping,
//! recording through a scripted transport timeline, audition and project files.

use super::tests::rack_only_core;
use super::*;
use crate::record::{InputQuantize, RecordedNote};
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{SongInput, TransportCommand, TransportState};
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, ControllerKind, Curve, DeviceKey, Note,
    SongCommand, TempoEvent,
};
use atomik_ui::{TransportAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const BAR: i64 = 4 * PPQ;

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(type_id: &str, k: &str) -> Command {
    Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    }
}

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    c.sink_mut().take();
    c
}

fn apply(c: &mut AppCore<RecordingSink>, cmd: Command) -> Vec<SinkCall> {
    c.apply_command(cmd).expect("command accepted");
    c.sink_mut().take()
}

fn note_clip(start: i64, notes: &[(i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(tick, key)| Note::new(tick, PPQ / 2, key, 0.8))
                .collect(),
        ),
    }
}

/// A core with synth `a` (id 1) on a track, the start-up calls taken.
fn with_track() -> AppCore<RecordingSink> {
    let mut c = core();
    apply(&mut c, add(SYNTH, "a"));
    apply(&mut c, song(SongCommand::AddTrack { device: key("a") }));
    c
}

fn songs(calls: &[SinkCall]) -> Vec<&SongInput> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Song(s) => Some(s),
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

fn kinds(calls: &[SinkCall]) -> Vec<&'static str> {
    calls
        .iter()
        .map(|c| match c {
            SinkCall::Reset { .. } => "reset",
            SinkCall::Insert { .. } => "insert",
            SinkCall::Remove(_) => "remove",
            SinkCall::Routing(_) => "routing",
            SinkCall::Param { .. } => "param",
            SinkCall::Samples { .. } => "samples",
            SinkCall::PlayerData { .. } => "player-data",
            SinkCall::Tempo(_) => "tempo",
            SinkCall::Song(_) => "song",
            SinkCall::Transport(_) => "transport",
            SinkCall::LiveNote { .. } => "live",
            SinkCall::PlayNote { .. } => "play",
            SinkCall::Recording(_) => "recording",
            SinkCall::Hold(_) => "hold",
            SinkCall::PreviewStart { .. } => "preview-start",
            SinkCall::PreviewStop => "preview-stop",
        })
        .collect()
}

fn param_id(c: &AppCore<RecordingSink>, param: &str) -> ParamId {
    c.registry()
        .get(SYNTH)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id
}

fn transport(c: &mut AppCore<RecordingSink>, action: TransportAction) -> Vec<SinkCall> {
    c.handle_action(UiAction::Transport(action));
    c.sink_mut().take()
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-song-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

// ------------------------------------------------------------------ song sync

#[test]
fn start_up_submits_the_song_and_the_transport_setup_after_the_rack() {
    let mut c = rack_only_core();
    let calls = c.sink_mut().take();
    assert_eq!(
        kinds(&calls),
        ["reset", "routing", "song", "transport", "transport"]
    );
    assert_eq!(
        songs(&calls)[0],
        &SongInput {
            tracks: vec![],
            tempo: vec![atomik_engine::SongTempo {
                tick: Ticks(0),
                tempo: Tempo::new(120.0).unwrap(),
                ramp: false,
            }],
            meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
            audio: Vec::new(),
            assets: Vec::new(),
        }
    );
    assert_eq!(
        transports(&calls),
        [
            TransportCommand::SetLoop {
                start: Ticks(0),
                end: Ticks(4 * BAR),
                enabled: false,
            },
            TransportCommand::Metronome {
                enabled: false,
                gain: 0.5,
            },
        ]
    );
    assert!(c.song_snapshot().is_some(), "a clone is kept for recording");
}

#[test]
fn adding_a_track_submits_the_song_with_that_track() {
    let mut c = core();
    let calls = apply(&mut c, add(SYNTH, "a"));
    assert_eq!(
        kinds(&calls),
        ["insert", "routing"],
        "no track yet: no song"
    );
    let calls = apply(&mut c, song(SongCommand::AddTrack { device: key("a") }));
    assert_eq!(kinds(&calls), ["song"]);
    let s = songs(&calls)[0];
    assert_eq!(s.tracks.len(), 1);
    assert_eq!(s.tracks[0].device, id(1));
    let socket = notes_input(c.registry().get(SYNTH).unwrap().descriptor).unwrap();
    assert_eq!(s.tracks[0].notes_socket, socket);
}

#[test]
fn clips_are_flattened_into_song_ticks_and_automation_uses_param_ids() {
    let mut c = with_track();
    let calls = apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(BAR, &[(0, 60), (PPQ, 62)]),
        }),
    );
    let s = songs(&calls)[0];
    let ticks: Vec<(i64, u8)> = s.tracks[0]
        .notes
        .iter()
        .map(|n| (n.tick.0, n.key))
        .collect();
    assert_eq!(ticks, [(BAR, 60), (BAR + PPQ, 62)]);
    apply(
        &mut c,
        song(SongCommand::AddAutomationLane {
            device: key("a"),
            param: "filter.cutoff".into(),
        }),
    );
    let calls = apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 1,
            clip: Clip {
                content: ClipContent::Automation(vec![
                    AutomationPoint {
                        tick: 0,
                        value: 300.0,
                        curve: Curve::Linear,
                    },
                    AutomationPoint {
                        tick: PPQ,
                        value: 900.0,
                        curve: Curve::Step,
                    },
                ]),
                ..note_clip(0, &[])
            },
        }),
    );
    let s = songs(&calls)[0];
    assert_eq!(s.tracks[0].automation.len(), 1);
    assert_eq!(
        s.tracks[0].automation[0].param,
        param_id(&c, "filter.cutoff")
    );
    assert_eq!(s.tracks[0].automation[0].points[0].tick, Ticks(0));
}

#[test]
fn tempo_changes_recompile_the_song_and_resend_the_loop() {
    let mut c = with_track();
    let calls = apply(&mut c, Command::SetTempo(90.0));
    assert_eq!(kinds(&calls), ["tempo", "song", "transport"]);
    assert!((songs(&calls)[0].tempo[0].tempo.bpm() - 90.0).abs() < 1e-12);
    assert!(matches!(
        transports(&calls)[..],
        [TransportCommand::SetLoop { .. }]
    ));
    // A tempo map edit reports TempoChanged and SongChanged: one compile.
    let calls = apply(
        &mut c,
        song(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 100.0),
            TempoEvent::step(BAR, 140.0),
        ])),
    );
    assert_eq!(kinds(&calls), ["tempo", "song", "transport"]);
    assert_eq!(songs(&calls)[0].tempo.len(), 2);
    let snap = c.song_snapshot().unwrap();
    assert!((snap.tempo_at(Ticks(BAR)).bpm() - 140.0).abs() < 1e-12);
}

#[test]
fn loop_edits_reach_the_engine_loop() {
    let mut c = with_track();
    let calls = apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (BAR, 3 * BAR),
            enabled: true,
        }),
    );
    assert_eq!(kinds(&calls), ["song", "transport"]);
    assert_eq!(
        transports(&calls),
        [TransportCommand::SetLoop {
            start: Ticks(BAR),
            end: Ticks(3 * BAR),
            enabled: true
        }]
    );
    // Unrelated song edits do not resend an unchanged loop.
    let calls = apply(&mut c, song(SongCommand::SetSongEnd(20 * BAR)));
    assert_eq!(kinds(&calls), ["song"]);
}

#[test]
fn muting_drops_the_track_and_solo_keeps_only_soloed_tracks() {
    let mut c = with_track();
    apply(&mut c, add(SYNTH, "b"));
    apply(&mut c, song(SongCommand::AddTrack { device: key("b") }));
    let flags = |muted, solo| {
        song(SongCommand::SetTrackFlags {
            device: key("a"),
            muted,
            solo,
            armed: false,
        })
    };
    let calls = apply(&mut c, flags(true, false));
    let devices: Vec<_> = songs(&calls)[0].tracks.iter().map(|t| t.device).collect();
    assert_eq!(devices, [id(2)]);
    let calls = apply(&mut c, flags(false, true));
    let devices: Vec<_> = songs(&calls)[0].tracks.iter().map(|t| t.device).collect();
    assert_eq!(devices, [id(1)]);
}

#[test]
fn undo_and_redo_of_song_edits_resubmit_the_song() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    assert!(c.undo());
    let calls = c.sink_mut().take();
    assert_eq!(kinds(&calls), ["song"]);
    assert!(songs(&calls)[0].tracks[0].notes.is_empty());
    assert!(c.redo());
    let calls = c.sink_mut().take();
    assert_eq!(songs(&calls)[0].tracks[0].notes.len(), 1);
}

#[test]
fn removing_a_device_with_a_track_submits_the_song_then_routing_then_removal() {
    let mut c = with_track();
    apply(&mut c, add(DELAY, "fx"));
    let calls = apply(&mut c, Command::RemoveDevice { key: key("a") });
    assert_eq!(kinds(&calls), ["song", "routing", "remove"]);
    assert!(songs(&calls)[0].tracks.is_empty());
    assert_eq!(calls[2], SinkCall::Remove(id(1)));
    // Undo: insert (new id), routing, then the song with the track again.
    assert!(c.undo());
    let calls = c.sink_mut().take();
    assert_eq!(kinds(&calls), ["insert", "routing", "song"]);
    assert_eq!(songs(&calls)[0].tracks[0].device, id(3));
    // Redo removes it again in the safe order.
    assert!(c.redo());
    assert_eq!(kinds(&c.sink_mut().take()), ["song", "routing", "remove"]);
}

#[test]
fn engine_errors_on_song_submission_are_reported() {
    let mut c = with_track();
    c.sink_mut().fail_next = Some(atomik_engine::EngineError::QueueFull);
    apply(&mut c, song(SongCommand::SetSongEnd(20 * BAR)));
    assert!(c.last_rejection().unwrap().contains("queue"));
}

// ------------------------------------------------------------------ transport

#[test]
fn transport_actions_map_to_engine_commands() {
    let mut c = with_track();
    assert_eq!(
        transports(&transport(&mut c, TransportAction::Play)),
        [TransportCommand::Play]
    );
    assert!(c.is_playing());
    assert_eq!(
        transports(&transport(&mut c, TransportAction::TogglePlay)),
        [TransportCommand::Stop]
    );
    assert!(!c.is_playing());
    assert_eq!(
        transports(&transport(&mut c, TransportAction::TogglePlay)),
        [TransportCommand::Play]
    );
    assert_eq!(
        transports(&transport(&mut c, TransportAction::Stop)),
        [TransportCommand::Stop]
    );
    assert_eq!(
        transports(&transport(&mut c, TransportAction::ReturnToZero)),
        [TransportCommand::Locate(Ticks(0))]
    );
    assert_eq!(
        transports(&transport(&mut c, TransportAction::Locate(3 * BAR))),
        [TransportCommand::Locate(Ticks(3 * BAR))]
    );
    assert_eq!(c.position(), 3 * BAR, "shown until the engine reports");
}

#[test]
fn toggle_loop_is_an_undoable_song_edit_that_reaches_the_engine() {
    let mut c = with_track();
    let calls = transport(&mut c, TransportAction::ToggleLoop);
    assert!(c.doc().song.loop_enabled);
    assert_eq!(kinds(&calls), ["song", "transport"]);
    assert_eq!(
        transports(&calls),
        [TransportCommand::SetLoop {
            start: Ticks(0),
            end: Ticks(4 * BAR),
            enabled: true
        }]
    );
    assert!(c.undo());
    assert!(!c.doc().song.loop_enabled);
    assert!(matches!(
        transports(&c.sink_mut().take())[..],
        [TransportCommand::SetLoop { enabled: false, .. }]
    ));
}

#[test]
fn the_metronome_toggles_with_the_settings_gain() {
    let mut c = with_track();
    assert!((c.metronome_gain() - 0.5).abs() < 1e-6);
    assert_eq!(
        transports(&transport(&mut c, TransportAction::ToggleMetronome)),
        [TransportCommand::Metronome {
            enabled: true,
            gain: 0.5
        }]
    );
    assert!(c.metronome());
    c.set_metronome_gain(0.25);
    assert_eq!(
        transports(&c.sink_mut().take()),
        [TransportCommand::Metronome {
            enabled: true,
            gain: 0.25
        }]
    );
    transport(&mut c, TransportAction::ToggleMetronome);
    assert!(!c.metronome());
}

#[test]
fn each_frame_shows_the_engine_position_in_the_views() {
    let mut c = with_track();
    c.handle_action(UiAction::Transport(TransportAction::Play));
    c.sink_mut().state = TransportState {
        playing: true,
        tick: 2 * BAR + 5,
        engine_frame: 4096,
        ..TransportState::default()
    };
    c.frame();
    assert_eq!(c.position(), 2 * BAR + 5);
    assert_eq!(c.arrangement().playhead(), 2 * BAR + 5);
    let shown = c.transport_bar().state();
    assert!(shown.playing && !shown.recording);
    assert_eq!(shown.position, 2 * BAR + 5);
}

// ------------------------------------------------------------------ recording

fn arm(c: &mut AppCore<RecordingSink>, device: &str, armed: bool) {
    c.apply_command(song(SongCommand::SetTrackFlags {
        device: key(device),
        muted: false,
        solo: false,
        armed,
    }))
    .unwrap();
}

#[test]
fn recording_needs_an_armed_track() {
    let mut c = with_track();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    assert!(transports(&calls).is_empty());
    assert!(!c.is_recording());
    assert!(c.last_rejection().unwrap().contains("arm"));
}

#[test]
fn armed_tracks_receive_live_notes() {
    let mut c = with_track();
    apply(&mut c, add(SYNTH, "b"));
    apply(&mut c, song(SongCommand::AddTrack { device: key("b") }));
    c.select(Some(key("a")));
    assert_eq!(c.notes_target().unwrap().0, id(1));
    arm(&mut c, "b", true);
    assert_eq!(c.notes_target().unwrap().0, id(2), "the armed track plays");
}

/// End to end: record, scripted engine timeline, stop, one undoable clip.
#[test]
fn recording_turns_live_notes_into_a_clip_on_stop() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    c.set_input_quantize(InputQuantize::SIXTEENTH);
    c.sink_mut().take();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    assert!(c.is_recording() && c.is_playing());
    assert_eq!(kinds(&calls), ["recording", "transport"]);
    assert_eq!(calls[0], SinkCall::Recording(vec![id(1)]));
    assert_eq!(transports(&calls), [TransportCommand::Play]);
    // The engine starts at engine frame 1000, song tick 0 (120 BPM: 1.5625 frames/tick).
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    c.sink_mut().recorded = vec![
        RecordedNote {
            device: id(1),
            kind: NoteKind::On {
                key: 45,
                velocity: 0.9,
            },
            frame: frame(PPQ + 300),
        },
        RecordedNote {
            device: id(1),
            kind: NoteKind::Off { key: 45 },
            frame: frame(2 * PPQ),
        },
        RecordedNote {
            device: id(1),
            kind: NoteKind::On {
                key: 48,
                velocity: 0.5,
            },
            frame: frame(BAR + 2 * PPQ),
        },
    ];
    c.frame();
    assert!(c.transport_bar().state().recording);
    // Stop at bar 2, beat 4: the held note closes there.
    c.sink_mut().state = TransportState {
        tick: BAR + 3 * PPQ,
        engine_frame: frame(BAR + 3 * PPQ),
        song_frame: frame(BAR + 3 * PPQ) - 1000,
        ..start
    };
    let undo_before = c.document().can_undo();
    let calls = transport(&mut c, TransportAction::Stop);
    assert!(!c.is_recording() && !c.is_playing());
    assert!(kinds(&calls).contains(&"recording"));
    assert!(transports(&calls).contains(&TransportCommand::Stop));
    let lane = &c.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips.len(), 1);
    let clip = &lane.clips[0];
    assert_eq!((clip.start, clip.length), (0, 2 * BAR));
    let ClipContent::Notes(notes) = &clip.content else {
        panic!("note clip")
    };
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].tick, PPQ, "quantized to 1/16");
    assert_eq!(notes[0].key, 45);
    assert!((notes[0].velocity - 0.9).abs() < 1e-6);
    assert_eq!(notes[1].tick, BAR + 2 * PPQ);
    assert!((notes[1].length - PPQ).abs() <= 1, "closed at the stop");
    // The new song (with the clip) reached the engine.
    assert_eq!(songs(&calls).last().unwrap().tracks[0].notes.len(), 2);
    // One undo step removes the take.
    assert!(undo_before);
    assert!(c.undo());
    assert!(c.doc().song.tracks[0].lanes[0].clips.is_empty());
}

/// With a pre-count the take starts with a count-in: the click counts, nothing played during it
/// is recorded, and the clip starts where the song does.
#[test]
fn recording_with_a_pre_count_records_nothing_before_the_song_starts() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    c.set_pre_count_bars(1);
    c.sink_mut().take();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    let plan = c
        .song_snapshot()
        .unwrap()
        .pre_roll(Ticks(0), 1, f64::from(c.doc().sample_rate))
        .unwrap();
    assert_eq!((plan.beats, plan.per_bar, plan.frames), (4, 4, 96_000));
    assert_eq!(
        transports(&calls),
        [TransportCommand::PlayWithPreRoll(plan)]
    );
    assert!(c.is_recording() && c.is_playing());
    // The engine counts: not playing yet, the song starts 96000 frames after engine frame 1000.
    let counting = TransportState {
        playing: false,
        engine_frame: 1000,
        pre_roll_frames: 96_000,
        pre_roll_beat: 1,
        ..TransportState::default()
    };
    c.sink_mut().state = counting;
    c.sink_mut().recorded = vec![RecordedNote {
        device: id(1),
        kind: NoteKind::On {
            key: 45,
            velocity: 0.9,
        },
        frame: 5000,
    }];
    c.frame();
    let shown = c.transport_bar().state();
    assert_eq!((shown.pre_count, shown.count_in), (1, 1));
    // The song starts at engine frame 97000; a note a beat in is recorded, the one before is not.
    let start = TransportState {
        playing: true,
        engine_frame: 97_000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 97_000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    c.sink_mut().recorded = vec![
        RecordedNote {
            device: id(1),
            kind: NoteKind::On {
                key: 48,
                velocity: 0.5,
            },
            frame: frame(PPQ),
        },
        RecordedNote {
            device: id(1),
            kind: NoteKind::Off { key: 48 },
            frame: frame(2 * PPQ),
        },
    ];
    c.frame();
    assert_eq!(c.transport_bar().state().count_in, 0);
    c.sink_mut().state = TransportState {
        tick: 3 * PPQ,
        engine_frame: frame(3 * PPQ),
        song_frame: frame(3 * PPQ) - 97_000,
        ..start
    };
    transport(&mut c, TransportAction::Stop);
    let clips = &c.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].start, 0, "the clip starts at bar 1");
    let ClipContent::Notes(notes) = &clips[0].content else {
        panic!("note clip")
    };
    assert_eq!(notes.len(), 1, "only the note after the count");
    assert_eq!(notes[0].key, 48);
    assert!((notes[0].tick - PPQ).abs() <= 1, "{}", notes[0].tick);
}

#[test]
fn the_pre_count_counts_in_from_the_position_and_not_when_already_playing() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    c.set_pre_count_bars(2);
    c.sink_mut().take();
    transport(&mut c, TransportAction::Locate(BAR));
    c.sink_mut().take();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    let plan = c.song_snapshot().unwrap().pre_roll(Ticks(BAR), 2, 48_000.0);
    assert_eq!(
        transports(&calls),
        [TransportCommand::PlayWithPreRoll(plan.unwrap())]
    );
    assert_eq!(plan.unwrap().beats, 8);
    // Already running: a take starts at once, with nothing to count in.
    transport(&mut c, TransportAction::Stop);
    transport(&mut c, TransportAction::Play);
    c.sink_mut().take();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    assert!(c.is_recording());
    assert!(transports(&calls).is_empty(), "{calls:?}");
}

#[test]
fn the_pre_count_toggle_returns_to_the_last_length_and_is_clamped() {
    let mut c = with_track();
    assert_eq!((c.pre_count_bars(), c.pre_count_toggled()), (0, 1));
    c.set_pre_count_bars(3);
    assert_eq!((c.pre_count_bars(), c.pre_count_toggled()), (3, 0));
    c.set_pre_count_bars(0);
    assert_eq!(c.pre_count_toggled(), 3, "back to the length it had");
    c.set_pre_count_bars(9);
    assert_eq!(c.pre_count_bars(), 4);
}

/// A track that enters at the top of a player stack records what is played into that entry (the
/// input notes, before the players), into its own clip.
#[test]
fn recording_through_a_player_stack_records_the_input_notes() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    apply(
        &mut c,
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: "atomik.arpeggiator".into(),
            target: atomik_model::CreateTarget::Selected(key("a")),
            options: atomik_model::CreateOptions::default(),
        }),
    );
    let arp = c
        .doc()
        .song
        .track(&key("a"))
        .unwrap()
        .notes_to
        .clone()
        .unwrap();
    let entry = c.device_id(&arp).unwrap();
    assert_ne!(entry, id(1));
    c.set_input_quantize(InputQuantize::SIXTEENTH);
    c.sink_mut().take();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    assert_eq!(calls[0], SinkCall::Recording(vec![entry]));
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    c.sink_mut().recorded = vec![
        RecordedNote {
            device: entry,
            kind: NoteKind::On {
                key: 45,
                velocity: 0.9,
            },
            frame: frame(PPQ),
        },
        RecordedNote {
            device: entry,
            kind: NoteKind::Off { key: 45 },
            frame: frame(2 * PPQ),
        },
    ];
    c.frame();
    c.sink_mut().state = TransportState {
        tick: BAR,
        engine_frame: frame(BAR),
        song_frame: frame(BAR) - 1000,
        ..start
    };
    transport(&mut c, TransportAction::Stop);
    let lane = &c.doc().song.track(&key("a")).unwrap().lanes[0];
    assert_eq!(
        lane.clips.len(),
        1,
        "the take lands on the instrument's track"
    );
    let ClipContent::Notes(notes) = &lane.clips[0].content else {
        panic!("note clip")
    };
    assert_eq!((notes.len(), notes[0].key), (1, 45));
}

/// With Direct Record on, the take records at the instrument, not at the top of the stack: the
/// sink taps the instrument's notes input, and the generated notes land in the track's clip.
#[test]
fn direct_record_records_the_notes_the_players_generate() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    apply(
        &mut c,
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: "atomik.arpeggiator".into(),
            target: atomik_model::CreateTarget::Selected(key("a")),
            options: atomik_model::CreateOptions::default(),
        }),
    );
    apply(
        &mut c,
        Command::Song(SongCommand::SetDirectRecord {
            device: key("a"),
            on: true,
        }),
    );
    c.set_input_quantize(InputQuantize::OFF);
    c.sink_mut().take();
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    assert_eq!(calls[0], SinkCall::Recording(vec![id(1)]), "the instrument");
    assert_eq!(c.sink_mut().note_taps, vec![id(1)]);
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    c.sink_mut().recorded = vec![
        RecordedNote {
            device: id(1),
            kind: NoteKind::On {
                key: 52,
                velocity: 0.5,
            },
            frame: frame(PPQ),
        },
        RecordedNote {
            device: id(1),
            kind: NoteKind::Off { key: 52 },
            frame: frame(PPQ + PPQ / 4),
        },
    ];
    c.frame();
    c.sink_mut().state = TransportState {
        tick: BAR,
        engine_frame: frame(BAR),
        song_frame: frame(BAR) - 1000,
        ..start
    };
    transport(&mut c, TransportAction::Stop);
    assert!(c.sink_mut().note_taps.is_empty(), "the tap comes off");
    let lane = &c.doc().song.track(&key("a")).unwrap().lanes[0];
    let ClipContent::Notes(notes) = &lane.clips[0].content else {
        panic!("note clip")
    };
    assert_eq!((notes.len(), notes[0].key), (1, 52));
    assert_eq!(notes[0].length, PPQ / 4);
}

/// A track that does not record Direct keeps recording the input notes.
#[test]
fn direct_record_off_taps_nothing() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    c.sink_mut().take();
    transport(&mut c, TransportAction::ToggleRecord);
    assert!(c.sink_mut().note_taps.is_empty());
}

fn with_stack(type_id: &str) -> AppCore<RecordingSink> {
    let mut c = with_track();
    apply(
        &mut c,
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: type_id.into(),
            target: atomik_model::CreateTarget::Selected(key("a")),
            options: atomik_model::CreateOptions::default(),
        }),
    );
    c.arrangement_mut().set_selected_track(Some(key("a")));
    c
}

fn lane_count(c: &AppCore<RecordingSink>) -> usize {
    c.doc().song.track(&key("a")).unwrap().note_lane_count()
}

#[test]
fn the_player_stack_commands_need_a_stack() {
    let mut c = with_track();
    c.arrangement_mut().set_selected_track(Some(key("a")));
    let revision = c.doc().song.clone();
    for f in [
        AppCore::toggle_direct_record,
        AppCore::fold_all_players,
        AppCore::bypass_all_players,
        AppCore::send_players_to_track,
    ] {
        f(&mut c);
        assert!(c.last_rejection().is_some_and(|m| m.contains("players")));
    }
    assert_eq!(*c.doc().song, *revision);
    assert_eq!(c.players_menu(), (false, false, false, false));
}

#[test]
fn direct_record_toggles_on_the_selected_stack_in_one_step() {
    let mut c = with_stack("atomik.arpeggiator");
    assert_eq!(c.players_menu(), (true, false, false, false));
    c.toggle_direct_record();
    assert!(c.doc().song.track(&key("a")).unwrap().direct_record);
    assert!(c.players_menu().1);
    c.toggle_direct_record();
    assert!(!c.doc().song.track(&key("a")).unwrap().direct_record);
    c.undo();
    assert!(
        c.doc().song.track(&key("a")).unwrap().direct_record,
        "one undo step"
    );
}

#[test]
fn fold_all_and_bypass_all_players_toggle_every_player_of_the_stack() {
    let mut c = with_stack("atomik.arpeggiator");
    apply(
        &mut c,
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: "atomik.note-filter".into(),
            target: atomik_model::CreateTarget::Selected(key("a")),
            options: atomik_model::CreateOptions::default(),
        }),
    );
    let t = c.doc().song.track(&key("a")).unwrap().clone();
    let stack = atomik_model::player_stack(c.doc(), &default_registry(), &t);
    assert_eq!(stack.len(), 2);
    c.fold_all_players();
    assert!(stack.iter().all(|k| c.doc().device(k).unwrap().folded));
    assert!(c.players_menu().2);
    c.fold_all_players();
    assert!(stack.iter().all(|k| !c.doc().device(k).unwrap().folded));
    c.bypass_all_players();
    let bypassed = |c: &AppCore<RecordingSink>| {
        stack
            .iter()
            .all(|k| c.doc().device(k).unwrap().params.get("bypass") == Some(&1.0))
    };
    assert!(bypassed(&c) && c.players_menu().3);
    c.undo();
    assert!(!bypassed(&c), "all of them in one undo step");
    c.bypass_all_players();
    c.bypass_all_players();
    assert!(!c.players_menu().3, "a second press releases them");
}

#[test]
fn send_players_to_track_writes_the_generated_notes_to_a_new_lane_in_one_step() {
    let mut c = with_stack("atomik.note-filter");
    let filter = c
        .doc()
        .song
        .track(&key("a"))
        .unwrap()
        .notes_to
        .clone()
        .unwrap();
    apply(
        &mut c,
        Command::SetParam {
            key: filter,
            param: "transpose".into(),
            value: 7.0,
            merge: None,
        },
    );
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 48), (PPQ, 52)]),
        }),
    );
    assert_eq!(lane_count(&c), 1);
    c.send_players_to_track();
    assert!(
        !c.last_rejection().is_some_and(|m| m.contains("failed")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(lane_count(&c), 2);
    let t = c.doc().song.track(&key("a")).unwrap();
    assert_eq!(t.lanes[1].name.as_deref(), Some("Players"));
    let ClipContent::Notes(notes) = &t.lanes[1].clips[0].content else {
        panic!("note clip")
    };
    let got: Vec<(i64, i64, u8)> = notes.iter().map(|n| (n.tick, n.length, n.key)).collect();
    assert_eq!(got, vec![(0, PPQ / 2, 55), (PPQ, PPQ / 2, 59)]);
    assert_eq!(
        t.lanes[1].clips[0].length % BAR,
        0,
        "rounded out to whole bars"
    );
    c.undo();
    assert_eq!(lane_count(&c), 1, "the lane and the clip are one undo step");
}

/// Pitch bend and a CC recorded with the notes land in the clip's controller lanes, reach the
/// engine's song input and undo with the take.
#[test]
fn recording_turns_live_controller_data_into_clip_lanes() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    c.sink_mut().take();
    transport(&mut c, TransportAction::ToggleRecord);
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    let ev = |kind, tick| RecordedNote {
        device: id(1),
        kind,
        frame: frame(tick),
    };
    c.sink_mut().recorded = vec![
        ev(NoteKind::PitchBend { value: 0.5 }, PPQ),
        ev(
            NoteKind::Cc {
                controller: 1,
                value: 1.0,
            },
            2 * PPQ,
        ),
    ];
    c.frame();
    c.sink_mut().state = TransportState {
        tick: BAR,
        engine_frame: frame(BAR),
        song_frame: frame(BAR) - 1000,
        ..start
    };
    let calls = transport(&mut c, TransportAction::Stop);
    let clip = &c.doc().song.tracks[0].lanes[0].clips[0];
    assert!(matches!(&clip.content, ClipContent::Notes(n) if n.is_empty()));
    let kinds: Vec<_> = clip.controllers.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [ControllerKind::PitchBend, ControllerKind::Cc(1)]);
    let lanes = &songs(&calls).last().unwrap().tracks[0].controllers;
    assert_eq!(lanes.len(), 2);
    assert!(c.undo());
    assert!(c.doc().song.tracks[0].lanes[0].clips.is_empty());
}

/// Recording over an enabled loop that wraps stacks the passes as takes of one group (one undo
/// step), the last pass playing; a take that never wraps stays a plain clip.
#[test]
fn recording_over_a_loop_stacks_the_passes_as_takes_of_one_group() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (0, BAR),
            enabled: true,
        }),
    );
    c.sink_mut().take();
    transport(&mut c, TransportAction::ToggleRecord);
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        loop_start: 0,
        loop_end: BAR,
        loop_enabled: true,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    // The first playing state anchors the take at pass 0.
    c.frame();
    let ev = |kind, tick| RecordedNote {
        device: id(1),
        kind,
        frame: frame(tick),
    };
    let on = |key| NoteKind::On { key, velocity: 0.8 };
    c.sink_mut().recorded = vec![
        ev(on(60), PPQ),
        ev(NoteKind::Off { key: 60 }, PPQ + PPQ / 2),
        ev(on(62), BAR + PPQ),
        ev(NoteKind::Off { key: 62 }, BAR + PPQ + PPQ / 2),
    ];
    // The second pass is under way: the transport wrapped once and is 2 beats into the loop.
    c.sink_mut().state = TransportState {
        tick: 2 * PPQ,
        engine_frame: frame(BAR + 2 * PPQ),
        song_frame: frame(2 * PPQ) - 1000,
        loop_count: 1,
        ..start
    };
    c.frame();
    let calls = transport(&mut c, TransportAction::Stop);
    let lane = &c.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.take_groups.len(), 1);
    let group = &lane.take_groups[0];
    assert_eq!(group.takes.len(), 2, "one take per pass");
    assert_eq!(group.picked(), Some(1), "the last pass plays");
    assert_eq!(lane.clips.len(), 1, "the takes are not on the lane");
    let key_of = |t: &atomik_model::Clip| match &t.content {
        ClipContent::Notes(n) => n[0].key,
        other => panic!("{other:?}"),
    };
    assert_eq!(group.takes.iter().map(key_of).collect::<Vec<_>>(), [60, 62]);
    // The engine got the picked pass only.
    let sent = songs(&calls);
    let notes = &sent.last().unwrap().tracks[0].notes;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].key, 62);
    // One undo step takes the whole recording away.
    assert!(c.undo());
    assert!(c.doc().song.tracks[0].lanes[0].take_groups.is_empty());
    assert!(c.doc().song.tracks[0].lanes[0].clips.is_empty());
}

#[test]
fn toggling_record_off_keeps_playing_and_writes_the_take() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    transport(&mut c, TransportAction::Play);
    transport(&mut c, TransportAction::ToggleRecord);
    let st = TransportState {
        playing: true,
        engine_frame: 0,
        ..TransportState::default()
    };
    c.sink_mut().state = st;
    c.sink_mut().recorded = vec![RecordedNote {
        device: id(1),
        kind: NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
        frame: 100,
    }];
    c.frame();
    c.sink_mut().state.tick = PPQ;
    let calls = transport(&mut c, TransportAction::ToggleRecord);
    assert!(!transports(&calls).contains(&TransportCommand::Stop));
    assert!(c.is_playing() && !c.is_recording());
    assert_eq!(c.doc().song.tracks[0].lanes[0].clips.len(), 1);
}

// ------------------------------------------------------------------ audition, piano roll

#[test]
fn audition_sends_live_notes_to_the_device() {
    let mut c = with_track();
    let socket = notes_input(c.registry().get(SYNTH).unwrap().descriptor).unwrap();
    c.handle_action(UiAction::Audition {
        device: key("a"),
        key: 64,
        on: true,
    });
    c.handle_action(UiAction::Audition {
        device: key("a"),
        key: 64,
        on: false,
    });
    let calls = c.sink_mut().take();
    assert_eq!(
        calls,
        [
            SinkCall::LiveNote {
                target: (id(1), socket),
                kind: NoteKind::On {
                    key: 64,
                    velocity: AUDITION_VELOCITY
                }
            },
            SinkCall::LiveNote {
                target: (id(1), socket),
                kind: NoteKind::Off { key: 64 }
            },
        ]
    );
    // Unknown devices are ignored.
    c.handle_action(UiAction::Audition {
        device: key("nope"),
        key: 64,
        on: true,
    });
    assert!(c.sink_mut().take().is_empty());
}

#[test]
fn piano_roll_actions_open_and_close_it() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    let clip = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.handle_action(UiAction::OpenPianoRoll(clip));
    assert_eq!(c.piano_roll_clip(), Some(clip));
    assert_eq!(c.piano_roll().clip(), Some(clip));
    c.handle_action(UiAction::ClosePianoRoll);
    assert_eq!(c.piano_roll_clip(), None);
    // The default target: the first note clip.
    assert!(c.open_piano_roll_default());
    assert_eq!(c.piano_roll_clip(), Some(clip));
    // Removing the clip closes the roll.
    c.apply_command(song(SongCommand::RemoveClip { id: clip }))
        .unwrap();
    assert_eq!(c.piano_roll_clip(), None);
}

#[test]
fn a_note_stepped_onto_in_the_piano_roll_sounds_and_never_hangs() {
    let mut c = with_track();
    let socket = notes_input(c.registry().get(SYNTH).unwrap().descriptor).unwrap();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60), (480, 64), (960, 67)]),
        }),
    );
    let clip = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.handle_action(UiAction::OpenPianoRoll(clip));
    c.piano_roll_mut().set_selected_notes(&[0]);
    c.sink_mut().take();
    let on = |k: u8| SinkCall::LiveNote {
        target: (id(1), socket),
        kind: NoteKind::On {
            key: k,
            velocity: AUDITION_VELOCITY,
        },
    };
    let off = |k: u8| SinkCall::LiveNote {
        target: (id(1), socket),
        kind: NoteKind::Off { key: k },
    };
    assert!(
        c.pane_command(Pane::PianoRoll, "editor.select_next_note", None)
            .is_some()
    );
    assert_eq!(c.sink_mut().take(), [on(64)]);
    // The clock releases it after a moment.
    c.piano_roll_tick(1.0);
    assert!(c.sink_mut().take().is_empty());
    c.piano_roll_tick(1.5);
    assert_eq!(c.sink_mut().take(), [off(64)]);
    // Closing the roll mid-note releases it.
    c.pane_command(Pane::PianoRoll, "editor.select_next_note", None);
    c.sink_mut().take();
    c.handle_action(UiAction::ClosePianoRoll);
    assert_eq!(c.sink_mut().take(), [off(67)]);
    c.piano_roll_tick(9.0);
    assert!(c.sink_mut().take().is_empty());
}

// ------------------------------------------------------------------ files

fn demo_project() -> &'static str {
    include_str!("../../../examples/projects/demo.atomik")
}

#[test]
fn opening_a_project_loads_the_song_and_submits_it() {
    let path = temp_path("demo.atomik");
    std::fs::write(&path, demo_project()).unwrap();
    // The demo is a v2 project with a console: it needs the full registry.
    let mut c = AppCore::new(RecordingSink::new());
    c.sink_mut().take();
    c.open(&path).unwrap();
    assert_eq!(c.doc().song.tracks.len(), 2);
    let calls = c.sink_mut().take();
    let s = songs(&calls);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].tracks.len(), 2);
    assert!(transports(&calls).contains(&TransportCommand::SetLoop {
        start: Ticks(0),
        end: Ticks(491_520),
        enabled: true
    }));
    assert_eq!(c.title(), "Atomik Rack \u{2014} demo.atomik");
    std::fs::remove_file(&path).ok();
}

#[test]
fn patches_open_as_song_less_projects() {
    let mut c = core();
    c.open_str(
        include_str!("../../../examples/patches/slice-demo.v2.json"),
        None,
    )
    .unwrap();
    assert!(c.doc().song.tracks.is_empty());
    assert_eq!(c.doc().devices.len(), 4);
}

#[test]
fn save_writes_a_project_that_reopens_identically_and_tracks_dirtiness() {
    let path = temp_path("saved.atomik");
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    assert!(c.title().ends_with(" *"));
    c.save_as(&path).unwrap();
    assert_eq!(c.title(), "Atomik Rack \u{2014} saved.atomik");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"atomik-project\""));
    let mut d = core();
    d.open(&path).unwrap();
    assert_eq!(d.doc(), c.doc());
    assert!(!d.is_dirty());
    std::fs::remove_file(&path).ok();
}

#[test]
fn save_of_a_patch_file_asks_for_a_project_name() {
    let path = temp_path("rack.json");
    std::fs::write(
        &path,
        include_str!("../../../examples/patches/slice-demo.v2.json"),
    )
    .unwrap();
    let mut c = core();
    c.open(&path).unwrap();
    assert!(matches!(c.save(), Err(AppError::NoPath)));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        include_str!("../../../examples/patches/slice-demo.v2.json"),
        "the patch is never overwritten with a project"
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn export_patch_writes_v2_without_the_song() {
    let path = temp_path("export.json");
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    c.export_patch(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"atomik-patch\""));
    assert!(!text.contains("\"song\""));
    let doc = atomik_model::load_patch(&text, c.registry()).unwrap();
    assert_eq!(doc.devices, c.doc().devices);
    assert!(c.is_dirty(), "exporting is not saving");
    assert_eq!(c.file(), None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn opening_a_new_file_stops_the_transport_and_recording() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    transport(&mut c, TransportAction::ToggleRecord);
    assert!(c.is_recording());
    c.new_document();
    assert!(!c.is_recording() && !c.is_playing());
    assert!(c.sink().calls.contains(&SinkCall::Recording(vec![])));
}

// ------------------------------------------------------------------ note lanes

/// Records one note over the first bar (the transport script of the test above) and stops;
/// `during` runs while the take is open.
fn record_a_note(c: &mut AppCore<RecordingSink>, during: impl FnOnce(&mut AppCore<RecordingSink>)) {
    transport(c, TransportAction::ToggleRecord);
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    c.sink_mut().recorded = vec![
        RecordedNote {
            device: id(1),
            kind: NoteKind::On {
                key: 45,
                velocity: 0.9,
            },
            frame: frame(PPQ),
        },
        RecordedNote {
            device: id(1),
            kind: NoteKind::Off { key: 45 },
            frame: frame(2 * PPQ),
        },
    ];
    c.frame();
    during(c);
    c.sink_mut().state = TransportState {
        tick: BAR,
        engine_frame: frame(BAR),
        song_frame: frame(BAR) - 1000,
        ..start
    };
    transport(c, TransportAction::Stop);
}

fn with_second_lane() -> AppCore<RecordingSink> {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddNoteLane {
            device: key("a"),
            name: None,
        }),
    );
    arm(&mut c, "a", true);
    c
}

#[test]
fn recording_lands_on_the_selected_note_lane() {
    let mut c = with_second_lane();
    c.arrangement_mut().set_selected_lane(&key("a"), 1);
    record_a_note(&mut c, |_| {});
    let track = &c.doc().song.tracks[0];
    assert!(track.lanes[0].clips.is_empty());
    assert_eq!(track.lanes[1].clips.len(), 1);
    assert!(c.undo());
    assert!(c.doc().song.tracks[0].lanes[1].clips.is_empty());
}

#[test]
fn recording_without_a_pick_lands_on_the_first_lane() {
    let mut c = with_second_lane();
    record_a_note(&mut c, |_| {});
    let track = &c.doc().song.tracks[0];
    assert_eq!(track.lanes[0].clips.len(), 1);
    assert!(track.lanes[1].clips.is_empty());
}

#[test]
fn recording_falls_back_to_the_first_lane_when_the_lane_was_deleted_meanwhile() {
    let mut c = with_second_lane();
    c.arrangement_mut().set_selected_lane(&key("a"), 1);
    record_a_note(&mut c, |c| {
        c.apply_command(song(SongCommand::RemoveNoteLane {
            device: key("a"),
            lane: 1,
        }))
        .unwrap();
    });
    let track = &c.doc().song.tracks[0];
    assert_eq!(track.lanes.len(), 1);
    assert_eq!(track.lanes[0].clips.len(), 1);
}

#[test]
fn a_clip_on_the_third_lane_opens_in_the_piano_roll_and_deleting_the_lane_closes_it() {
    let mut c = with_track();
    for _ in 0..2 {
        apply(
            &mut c,
            song(SongCommand::AddNoteLane {
                device: key("a"),
                name: None,
            }),
        );
    }
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 2,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    let clip = c.doc().song.tracks[0].lanes[2].clips[0].id;
    c.handle_action(UiAction::OpenPianoRoll(clip));
    assert_eq!(c.piano_roll_clip(), Some(clip));
    c.apply_command(song(SongCommand::RemoveNoteLane {
        device: key("a"),
        lane: 2,
    }))
    .unwrap();
    assert_eq!(c.piano_roll_clip(), None);
}

// ---- record mode: layer ---------------------------------------------------------------------

fn lane_clips(c: &AppCore<RecordingSink>) -> Vec<Clip> {
    c.doc().song.tracks[0].lanes[0].clips.clone()
}

fn note_keys(clip: &Clip) -> Vec<u8> {
    match &clip.content {
        ClipContent::Notes(n) => n.iter().map(|n| n.key).collect(),
        other => panic!("{other:?}"),
    }
}

/// Loop recording in Layer mode makes one clip that holds every pass, not a take group.
#[test]
fn layer_mode_merges_the_passes_of_a_loop_recording_into_one_clip() {
    let mut c = with_track();
    c.set_record_mode(crate::record_over::RecordMode::Layer);
    arm(&mut c, "a", true);
    apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (0, BAR),
            enabled: true,
        }),
    );
    c.sink_mut().take();
    transport(&mut c, TransportAction::ToggleRecord);
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        loop_start: 0,
        loop_end: BAR,
        loop_enabled: true,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    c.sink_mut().state = start;
    c.frame();
    let ev = |kind, tick| RecordedNote {
        device: id(1),
        kind,
        frame: frame(tick),
    };
    let on = |key| NoteKind::On { key, velocity: 0.8 };
    c.sink_mut().recorded = vec![
        ev(on(60), PPQ),
        ev(NoteKind::Off { key: 60 }, PPQ + PPQ / 2),
        ev(on(62), BAR + 2 * PPQ),
        ev(NoteKind::Off { key: 62 }, BAR + 2 * PPQ + PPQ / 2),
    ];
    c.sink_mut().state = TransportState {
        tick: 3 * PPQ,
        engine_frame: frame(BAR + 3 * PPQ),
        song_frame: frame(3 * PPQ) - 1000,
        loop_count: 1,
        ..start
    };
    c.frame();
    let calls = transport(&mut c, TransportAction::Stop);
    let lane = &c.doc().song.tracks[0].lanes[0];
    assert!(lane.take_groups.is_empty(), "no take group in Layer mode");
    assert_eq!(lane.clips.len(), 1);
    assert_eq!(note_keys(&lane.clips[0]), [60, 62], "every pass sounds");
    let notes = &songs(&calls).last().unwrap().tracks[0].notes;
    assert_eq!(notes.iter().map(|n| n.key).collect::<Vec<_>>(), [60, 62]);
    assert!(c.undo());
    assert!(
        lane_clips(&c).is_empty(),
        "one undo step takes the whole take away"
    );
}

/// Recording over an existing clip in Layer mode adds the notes to it and keeps the old ones.
#[test]
fn layer_mode_overdubs_into_the_clip_under_the_take() {
    let mut c = with_track();
    c.set_record_mode(crate::record_over::RecordMode::Layer);
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 50), (3 * PPQ, 51)]),
        }),
    );
    let old = lane_clips(&c)[0].id;
    arm(&mut c, "a", true);
    record_a_note(&mut c, |_| {});
    let clips = lane_clips(&c);
    assert_eq!(clips.len(), 1, "merged, not stacked");
    assert_eq!(clips[0].id, old);
    assert_eq!(note_keys(&clips[0]), [50, 45, 51]);
    assert!(c.undo());
    let clips = lane_clips(&c);
    assert_eq!(clips.len(), 1);
    assert_eq!(
        note_keys(&clips[0]),
        [50, 51],
        "the old notes are as they were"
    );
}

/// Replace mode is what it always was: the take is a new clip beside the old one, which still
/// hides it where it starts later.
#[test]
fn replace_mode_adds_a_plain_clip_and_a_later_starting_clip_still_hides_it() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(2 * PPQ, &[(0, 50)]),
        }),
    );
    arm(&mut c, "a", true);
    record_a_note(&mut c, |_| {});
    assert_eq!(lane_clips(&c).len(), 2);
    let notes = atomik_model::song::flatten_notes(&c.doc().song.tracks[0]);
    assert_eq!(
        notes.iter().map(|n| n.2).collect::<Vec<_>>(),
        [45, 50],
        "the recorded note at beat 2 plays; the older clip hides the rest of the take"
    );
}

/// Without a clip under its start, a Layer take plays over the clip that starts inside it.
#[test]
fn layer_mode_without_a_clip_under_the_take_engulfs_the_clip_that_starts_inside_it() {
    let mut c = with_track();
    c.set_record_mode(crate::record_over::RecordMode::Layer);
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(2 * PPQ, &[(0, 50), (2 * PPQ, 51)]),
        }),
    );
    arm(&mut c, "a", true);
    record_a_note(&mut c, |_| {});
    let clips = lane_clips(&c);
    assert_eq!(clips.len(), 2);
    let notes = atomik_model::song::flatten_notes(&c.doc().song.tracks[0]);
    // The take is bar 1 (a note at beat 2); the old clip (beats 3 to 7) now starts at bar 2.
    assert_eq!(notes.iter().map(|n| n.2).collect::<Vec<_>>(), [45, 51]);
    assert!(c.undo());
    assert_eq!(lane_clips(&c).len(), 1);
}

// ---- keep events in clip --------------------------------------------------------------------

fn with_clip_and_policy(
    policy: crate::record_over::KeepEvents,
) -> (AppCore<RecordingSink>, ClipId) {
    let mut c = with_track();
    c.set_keep_events(policy);
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    let id = lane_clips(&c)[0].id;
    (c, id)
}

fn set_notes(id: ClipId, notes: Vec<Note>, merge: Option<u64>) -> UiAction {
    UiAction::Command(song(SongCommand::SetClipContent {
        id,
        content: ClipContent::Notes(notes.into()),
        merge,
    }))
}

fn tick_len(c: &AppCore<RecordingSink>) -> Vec<(i64, i64)> {
    lane_clips(c).iter().map(|c| (c.start, c.length)).collect()
}

#[test]
fn keep_leaves_a_note_past_the_clip_end_where_it_is() {
    let (mut c, id) = with_clip_and_policy(crate::record_over::KeepEvents::Keep);
    c.handle_action(set_notes(
        id,
        vec![
            Note::new(0, PPQ, 60, 0.8),
            Note::new(BAR + PPQ, PPQ, 61, 0.8),
        ],
        None,
    ));
    assert_eq!(tick_len(&c), [(0, BAR)]);
    assert_eq!(note_keys(&lane_clips(&c)[0]), [60, 61]);
}

#[test]
fn expand_lengthens_the_clip_for_a_drawn_note_in_the_same_undo_step() {
    let (mut c, id) = with_clip_and_policy(crate::record_over::KeepEvents::Expand);
    c.handle_action(set_notes(
        id,
        vec![
            Note::new(0, PPQ, 60, 0.8),
            Note::new(BAR + PPQ, PPQ, 61, 0.8),
        ],
        None,
    ));
    assert_eq!(
        tick_len(&c),
        [(0, 2 * BAR)],
        "to the bar line after the note"
    );
    assert_eq!(note_keys(&lane_clips(&c)[0]), [60, 61]);
    assert!(c.undo());
    assert_eq!(tick_len(&c), [(0, BAR)]);
    assert_eq!(note_keys(&lane_clips(&c)[0]), [60], "one undo reverts both");
}

#[test]
fn expand_waits_for_the_end_of_a_drag_and_joins_its_undo_step() {
    let (mut c, id) = with_clip_and_policy(crate::record_over::KeepEvents::Expand);
    for tick in [PPQ, BAR / 2, BAR + PPQ] {
        c.handle_action(set_notes(id, vec![Note::new(tick, PPQ, 60, 0.8)], Some(7)));
    }
    assert_eq!(tick_len(&c), [(0, BAR)], "nothing happens mid-drag");
    c.handle_action(UiAction::EndMerge);
    assert_eq!(tick_len(&c), [(0, 2 * BAR)]);
    assert!(c.undo());
    assert_eq!(tick_len(&c), [(0, BAR)]);
    let first_tick = match &lane_clips(&c)[0].content {
        ClipContent::Notes(n) => n[0].tick,
        _ => unreachable!(),
    };
    assert_eq!(
        first_tick, 0,
        "the whole drag and the expansion are one step"
    );
}

#[test]
fn expand_follows_a_paste_that_lands_past_the_clip_end() {
    let (mut c, id) = with_clip_and_policy(crate::record_over::KeepEvents::Expand);
    let snapshot = atomik_model::NoteSnapshot {
        notes: vec![Note::new(0, PPQ, 70, 0.8)],
        span: PPQ,
    };
    apply(
        &mut c,
        Command::Workflow(atomik_model::WorkflowCommand::PasteNotes {
            clip: id,
            at_tick: BAR + 2 * PPQ,
            notes: snapshot,
        }),
    );
    assert_eq!(tick_len(&c), [(0, 2 * BAR)]);
    assert!(c.undo());
    assert_eq!(tick_len(&c), [(0, BAR)]);
    assert_eq!(note_keys(&lane_clips(&c)[0]), [60]);
}

#[test]
fn new_clip_moves_the_outside_notes_to_a_clip_on_the_next_bar() {
    let (mut c, id) = with_clip_and_policy(crate::record_over::KeepEvents::NewClip);
    c.handle_action(set_notes(
        id,
        vec![
            Note::new(0, PPQ, 60, 0.8),
            Note::new(BAR + PPQ, PPQ, 61, 0.8),
            Note::new(BAR + 2 * PPQ, PPQ, 62, 0.8),
        ],
        None,
    ));
    let clips = lane_clips(&c);
    assert_eq!(clips.len(), 2);
    assert_eq!(tick_len(&c), [(0, BAR), (BAR, BAR)]);
    assert_eq!(note_keys(&clips[0]), [60]);
    assert_eq!(note_keys(&clips[1]), [61, 62]);
    match &clips[1].content {
        ClipContent::Notes(n) => assert_eq!(n[0].tick, PPQ, "rebased to the new clip"),
        _ => unreachable!(),
    }
    assert!(c.undo());
    assert_eq!(lane_clips(&c).len(), 1);
    assert_eq!(note_keys(&lane_clips(&c)[0]), [60]);
}

#[test]
fn a_move_that_keeps_every_note_inside_the_clip_changes_nothing_under_any_policy() {
    for policy in crate::record_over::KeepEvents::ALL {
        let (mut c, id) = with_clip_and_policy(policy);
        c.handle_action(set_notes(id, vec![Note::new(PPQ, PPQ, 60, 0.8)], Some(1)));
        c.handle_action(UiAction::EndMerge);
        assert_eq!(tick_len(&c), [(0, BAR)], "{policy:?}");
    }
}

#[test]
fn moving_the_loop_by_bars_keeps_its_length_in_one_undo_step() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (BAR, 3 * BAR),
            enabled: true,
        }),
    );
    c.move_loop(2, false);
    assert_eq!(c.doc().song.loop_range, (3 * BAR, 5 * BAR));
    assert!(c.doc().song.loop_enabled, "the flag stays");
    c.move_loop(-1, false);
    assert_eq!(c.doc().song.loop_range, (2 * BAR, 4 * BAR));
    // Undo takes back one move.
    assert!(c.undo());
    assert_eq!(c.doc().song.loop_range, (3 * BAR, 5 * BAR));
    // It does not go before the first bar: the move is not made.
    c.move_loop(-5, false);
    assert_eq!(c.doc().song.loop_range, (3 * BAR, 5 * BAR));
}

#[test]
fn changing_the_loop_length_moves_the_right_end_and_keeps_one_bar() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (BAR, 3 * BAR),
            enabled: false,
        }),
    );
    c.move_loop(1, true);
    assert_eq!(c.doc().song.loop_range, (BAR, 4 * BAR));
    c.move_loop(-1, true);
    c.move_loop(-1, true);
    assert_eq!(c.doc().song.loop_range, (BAR, 2 * BAR));
    c.move_loop(-1, true);
    assert_eq!(c.doc().song.loop_range, (BAR, 2 * BAR), "one bar stays");
    assert!(!c.doc().song.loop_enabled);
}

#[test]
fn all_notes_off_reaches_every_device_that_takes_notes() {
    let mut c = with_track();
    apply(&mut c, add(SYNTH, "b"));
    c.sink_mut().take();
    c.all_notes_off();
    let calls = c.sink_mut().take();
    let offs: Vec<_> = calls
        .iter()
        .filter_map(|call| match call {
            SinkCall::LiveNote {
                target,
                kind: NoteKind::AllOff,
            } => Some(*target),
            _ => None,
        })
        .collect();
    assert_eq!(offs.len(), 2, "{calls:?}");
    assert_ne!(offs[0], offs[1]);
}

// ------------------------------------------------------------------ transport bar controls

#[test]
fn stop_returns_to_where_playback_started_and_a_second_stop_goes_to_the_start() {
    let mut c = with_track();
    // Off by default: Stop leaves the playhead where it ran to.
    transport(&mut c, TransportAction::Locate(4 * BAR));
    transport(&mut c, TransportAction::Play);
    transport(&mut c, TransportAction::Stop);
    assert_eq!(c.position(), 4 * BAR);
    c.set_return_to_start_on_stop(true);
    transport(&mut c, TransportAction::Play);
    assert_eq!(c.last_start(), Some(4 * BAR));
    c.sink_mut().state = TransportState {
        playing: true,
        tick: 6 * BAR,
        engine_frame: 5000,
        ..TransportState::default()
    };
    c.frame();
    assert_eq!(c.position(), 6 * BAR);
    transport(&mut c, TransportAction::Stop);
    assert_eq!(c.position(), 4 * BAR, "back to where it started");
    transport(&mut c, TransportAction::Stop);
    assert_eq!(c.position(), 0, "the second Stop goes to the start");
}

#[test]
fn tap_tempo_sets_the_tempo_at_the_playhead_in_one_undo_step() {
    let mut c = with_track();
    let before = c.doc().song.tempo[0].bpm;
    for t in [10.0, 10.6, 11.2, 11.8] {
        c.transport_frame(t);
        transport(&mut c, TransportAction::TapTempo);
    }
    let bpm = c.doc().song.tempo[0].bpm;
    assert!((bpm - 100.0).abs() < 0.5, "{bpm}");
    assert!(c.undo());
    assert_eq!(c.doc().song.tempo[0].bpm, before, "one step for the taps");
    // A pause of two seconds starts a new sequence: the first tap alone changes nothing.
    c.transport_frame(20.0);
    transport(&mut c, TransportAction::TapTempo);
    assert_eq!(c.doc().song.tempo[0].bpm, before);
}

#[test]
fn nudging_and_typing_a_tempo_edit_the_event_at_the_playhead() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 120.0),
            TempoEvent::step(BAR, 90.0),
        ])),
    );
    transport(&mut c, TransportAction::Locate(BAR + 10));
    c.nudge_tempo(1.0);
    let tempo = &c.doc().song.tempo;
    assert_eq!((tempo[0].bpm, tempo[1].bpm), (120.0, 91.0));
    transport(&mut c, TransportAction::Locate(0));
    c.nudge_tempo(-1.0);
    assert_eq!(c.doc().song.tempo[0].bpm, 119.0);
}

#[test]
fn the_click_slider_and_q_rec_reach_the_core() {
    let mut c = with_track();
    c.sink_mut().take();
    c.handle_action(UiAction::SetMetronomeGain(0.25));
    assert!((c.metronome_gain() - 0.25).abs() < 1e-6);
    assert!(!c.sink_mut().take().is_empty(), "the engine hears it");
    c.handle_action(UiAction::SetInputQuantize(Some(PPQ / 6)));
    assert_eq!(c.input_quantize(), InputQuantize::on(PPQ / 6));
    c.handle_action(UiAction::SetInputQuantize(None));
    assert_eq!(c.input_quantize(), InputQuantize::OFF);
}

#[test]
fn bars_and_locators_move_the_playhead() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (2 * BAR, 5 * BAR),
            enabled: false,
        }),
    );
    transport(&mut c, TransportAction::Bars(3));
    assert_eq!(c.position(), 3 * BAR);
    transport(&mut c, TransportAction::GoLocator(true));
    assert_eq!(c.position(), 5 * BAR);
    transport(&mut c, TransportAction::GoLocator(false));
    assert_eq!(c.position(), 2 * BAR);
}

/// A recording shows its notes as they are played, Delete throws them away while it runs on,
/// and the clip holds only what came after.
#[test]
fn recording_previews_the_take_and_delete_discards_the_notes_so_far() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    transport(&mut c, TransportAction::ToggleRecord);
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    let note = |key: u8, at: i64, on: bool| RecordedNote {
        device: id(1),
        kind: if on {
            NoteKind::On { key, velocity: 0.8 }
        } else {
            NoteKind::Off { key }
        },
        frame: frame(at),
    };
    c.sink_mut().state = TransportState {
        tick: 2 * PPQ,
        engine_frame: frame(2 * PPQ),
        song_frame: frame(2 * PPQ) - 1000,
        ..start
    };
    c.sink_mut().recorded = vec![note(45, PPQ, true), note(45, PPQ + PPQ / 2, false)];
    c.frame();
    let ghosts = c.take_preview().expect("recording");
    assert_eq!(ghosts.len(), 1);
    assert_eq!(ghosts[0].device, key("a"));
    assert_eq!(ghosts[0].notes.len(), 1);
    assert_eq!(c.arrangement().take_ghosts(), ghosts.as_slice());
    // Delete: the notes so far are gone, the take and the transport run on.
    let handled = c.pane_command(Pane::Arrangement, "edit.delete", None);
    assert!(handled.is_some());
    assert!(c.is_recording() && c.is_playing());
    assert!(c.take_preview().unwrap()[0].notes.is_empty());
    c.sink_mut().state = TransportState {
        tick: 4 * PPQ,
        engine_frame: frame(4 * PPQ),
        song_frame: frame(4 * PPQ) - 1000,
        ..start
    };
    c.sink_mut().recorded = vec![note(50, 3 * PPQ, true), note(50, 3 * PPQ + PPQ / 2, false)];
    c.frame();
    transport(&mut c, TransportAction::Stop);
    let clips = &c.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 1);
    let ClipContent::Notes(notes) = &clips[0].content else {
        panic!("note clip")
    };
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].key, 50);
    // No ghost once the take is over.
    assert!(c.arrangement().take_ghosts().is_empty());
}

#[test]
fn stopping_a_take_with_nothing_captured_makes_no_clip() {
    let mut c = with_track();
    arm(&mut c, "a", true);
    transport(&mut c, TransportAction::ToggleRecord);
    c.sink_mut().state = TransportState {
        playing: true,
        tick: BAR,
        engine_frame: 1000,
        ..TransportState::default()
    };
    c.frame();
    transport(&mut c, TransportAction::Stop);
    assert!(c.doc().song.tracks[0].lanes[0].clips.is_empty());
}

// ---- note lane workflows ---------------------------------------------------------------------

fn played_by_a(c: &AppCore<RecordingSink>) -> Vec<(i64, i64, u8, f32)> {
    atomik_model::song::flatten_notes_with(&c.doc().song.tracks[0], false)
}

#[test]
fn merge_note_lanes_is_one_undo_step_and_the_track_plays_the_same() {
    let mut c = with_second_lane();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60)]),
        }),
    );
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 1,
            clip: note_clip(0, &[(PPQ, 64)]),
        }),
    );
    let before = played_by_a(&c);
    assert_eq!(before.len(), 2);
    c.arrangement_mut().set_selected_track(Some(key("a")));
    c.arrangement_mut().set_selected_lane(&key("a"), 1);
    let _ = c.pane_command(Pane::Arrangement, "sequencer.merge_note_lanes", None);
    assert_eq!(c.doc().song.tracks[0].note_lane_count(), 1);
    assert_eq!(played_by_a(&c), before);
    assert_eq!(c.arrangement().record_lane(&key("a")), 0);
    assert!(c.undo());
    assert_eq!(c.doc().song.tracks[0].note_lane_count(), 2);
    assert_eq!(played_by_a(&c), before);
}

#[test]
fn explode_notes_selects_the_new_clips_and_extract_moves_notes() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(0, &[(0, 60), (PPQ, 64), (2 * PPQ, 67)]),
        }),
    );
    let before = played_by_a(&c);
    c.arrangement_mut().set_selected_clips(&[ClipId(1)]);
    let _ = c.pane_command(Pane::Arrangement, "sequencer.explode_notes", None);
    assert_eq!(c.doc().song.tracks[0].note_lane_count(), 4);
    assert_eq!(played_by_a(&c), before);
    assert_eq!(
        c.arrangement().selected_clips(),
        vec![ClipId(2), ClipId(3), ClipId(4)]
    );
    assert!(c.undo());
    assert_eq!(c.doc().song.tracks[0].note_lane_count(), 1);
    c.apply_command(Command::Workflow(
        atomik_model::WorkflowCommand::ExtractNotes {
            clip: ClipId(1),
            selection: vec![1],
            mode: atomik_model::ExtractMode::Move,
            to: None,
        },
    ))
    .unwrap();
    assert_eq!(c.doc().song.tracks[0].note_lane_count(), 2);
    assert_eq!(played_by_a(&c), before);
    assert_eq!(c.arrangement().selected_clips(), vec![ClipId(2)]);
}

// ---- dub and alt -----------------------------------------------------------------------------

fn armed_track() -> AppCore<RecordingSink> {
    let mut c = with_track();
    arm(&mut c, "a", true);
    c
}

fn lanes_of_a(c: &AppCore<RecordingSink>) -> usize {
    c.doc().song.tracks[0].note_lane_count()
}

#[test]
fn dub_adds_a_lane_per_armed_note_track_selects_it_and_is_one_undo_step() {
    let mut c = armed_track();
    transport(&mut c, TransportAction::Dub);
    assert_eq!(lanes_of_a(&c), 2);
    assert_eq!(c.arrangement().record_lane(&key("a")), 1);
    transport(&mut c, TransportAction::Dub);
    assert_eq!(
        (lanes_of_a(&c), c.arrangement().record_lane(&key("a"))),
        (3, 2)
    );
    assert!(c.undo());
    assert_eq!(lanes_of_a(&c), 2);
    assert!(c.undo());
    assert_eq!(lanes_of_a(&c), 1);
}

#[test]
fn dub_without_an_armed_note_track_is_refused_with_a_message() {
    let mut c = with_track();
    transport(&mut c, TransportAction::Dub);
    assert_eq!(lanes_of_a(&c), 1);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("armed note track"))
    );
}

#[test]
fn alt_mutes_the_lane_of_the_last_pass_in_the_same_undo_step() {
    let mut c = armed_track();
    transport(&mut c, TransportAction::Alt);
    let track = &c.doc().song.tracks[0];
    assert_eq!(track.note_lane_count(), 2);
    assert!(track.lanes[0].muted && !track.lanes[1].muted);
    assert!(c.undo());
    let track = &c.doc().song.tracks[0];
    assert_eq!(track.note_lane_count(), 1);
    assert!(!track.lanes[0].muted);
    // Dub leaves the layers sounding.
    transport(&mut c, TransportAction::Dub);
    assert!(c.doc().song.tracks[0].lanes.iter().all(|l| !l.muted));
}

#[test]
fn dub_in_the_middle_of_a_take_splits_it_between_the_layers_without_losing_notes() {
    let mut c = armed_track();
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    record_a_note(&mut c, |c| {
        // The note of the first half was fed by the frame before; the dub ends that take and
        // starts the next on the new lane, which takes the note played after the key press.
        c.sink_mut().state = TransportState {
            tick: 2 * PPQ,
            engine_frame: frame(2 * PPQ),
            song_frame: frame(2 * PPQ) - 1000,
            playing: true,
            ..TransportState::default()
        };
        transport(c, TransportAction::Dub);
        c.sink_mut().recorded = vec![
            RecordedNote {
                device: id(1),
                kind: NoteKind::On {
                    key: 52,
                    velocity: 0.9,
                },
                frame: frame(3 * PPQ),
            },
            RecordedNote {
                device: id(1),
                kind: NoteKind::Off { key: 52 },
                frame: frame(3 * PPQ + PPQ / 2),
            },
        ];
        c.frame();
    });
    let track = &c.doc().song.tracks[0];
    assert_eq!(track.note_lane_count(), 2);
    let keys = |lane: usize| -> Vec<u8> {
        track.lanes[lane]
            .clips
            .iter()
            .flat_map(|c| match &c.content {
                ClipContent::Notes(n) => n.iter().map(|n| n.key).collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect()
    };
    assert_eq!((keys(0), keys(1)), (vec![45], vec![52]));
}

fn only_clip(c: &AppCore<RecordingSink>) -> Clip {
    c.document().doc().song.track(&key("a")).unwrap().lanes[0].clips[0].clone()
}

fn stretch(c: &mut AppCore<RecordingSink>, percent: f64, from_start: bool) {
    let id = only_clip(c).id;
    c.handle_action(UiAction::StretchStep {
        ids: vec![id],
        percent,
        from_start,
    });
}

#[test]
fn a_stretch_preview_follows_the_pointer_from_the_original_and_is_one_undo_step() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(BAR, &[(0, 60), (PPQ, 62)]),
        }),
    );
    let undo_before = c.document().undo_depth();
    // Twice as long, then back to a quarter longer: each step is from the original clip.
    stretch(&mut c, 50.0, false);
    assert_eq!(only_clip(&c).length, 2 * BAR);
    stretch(&mut c, 80.0, false);
    assert_eq!(only_clip(&c).length, BAR * 100 / 80);
    // The preview is no undo step and the engine got the song each time.
    assert_eq!(c.document().undo_depth(), undo_before);
    assert!(
        c.sink_mut()
            .take()
            .iter()
            .any(|call| matches!(call, SinkCall::Song(_)))
    );
    c.handle_action(UiAction::StretchEnd { commit: true });
    assert_eq!(c.document().undo_depth(), undo_before + 1);
    assert_eq!(only_clip(&c).length, BAR * 100 / 80);
    // One undo gives the clip back.
    c.handle_action(UiAction::Undo);
    let clip = only_clip(&c);
    assert_eq!((clip.start, clip.length), (BAR, BAR));
}

#[test]
fn cancelling_a_stretch_puts_the_clip_back_and_a_later_command_accepts_the_preview() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip: note_clip(BAR, &[(0, 60)]),
        }),
    );
    let undo_before = c.document().undo_depth();
    stretch(&mut c, 50.0, true);
    assert_eq!(only_clip(&c).start, 0);
    c.handle_action(UiAction::StretchEnd { commit: false });
    let clip = only_clip(&c);
    assert_eq!((clip.start, clip.length), (BAR, BAR));
    assert_eq!(c.document().undo_depth(), undo_before);
    // Another command while a preview is open keeps the preview as its own step first.
    stretch(&mut c, 50.0, false);
    apply(&mut c, song(SongCommand::SetSongEnd(8 * BAR)));
    assert_eq!(c.document().undo_depth(), undo_before + 2);
    assert_eq!(only_clip(&c).length, 2 * BAR);
}

#[test]
fn a_refused_insert_or_remove_at_the_locators_says_why() {
    let mut c = with_track();
    apply(
        &mut c,
        song(SongCommand::SetLoop {
            range: (200 * BAR, 201 * BAR),
            enabled: true,
        }),
    );
    // Nothing is at or after the locators (far past the song): the command is refused and the reason is shown, not lost.
    for id in ["sequencer.insert_bars", "sequencer.remove_bars"] {
        let taken = c.pane_command(Pane::Arrangement, id, None);
        assert!(taken.is_some(), "{id} is the arrangement's");
        let message = c.last_rejection().expect("a visible message");
        assert!(message.contains("nothing"), "{id}: {message}");
    }
}
