//! `AppCore` tests with the recording sink: song sync, transport mapping,
//! recording through a scripted transport timeline, audition and project files.

use super::tests::rack_only_core;
use super::*;
use crate::record::{InputQuantize, RecordedNote};
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{SongInput, TransportCommand, TransportState};
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Note, SongCommand,
    TempoEvent,
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
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(tick, key)| Note {
                    tick,
                    length: PPQ / 2,
                    key,
                    velocity: 0.8,
                })
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
            SinkCall::Recording(_) => "recording",
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
            }],
            beats_per_bar: 4,
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
            TempoEvent {
                tick: 0,
                bpm: 100.0,
            },
            TempoEvent {
                tick: BAR,
                bpm: 140.0,
            },
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
    c.set_input_quantize(InputQuantize::Sixteenth);
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
