//! The Piano Keys notes in the core: they go to the same target as notes from a MIDI keyboard
//! and a take records them the same way.

use super::tests::rack_only_core;
use super::*;
use crate::record::RecordedNote;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::{NoteKind, PPQ};
use atomik_engine::TransportState;
use atomik_model::{ClipContent, Command, DeviceKey, Note, SongCommand};
use atomik_ui::{TransportAction, UiAction};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn with_armed_track() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    for name in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key(name)),
        })
        .unwrap();
        c.apply_command(Command::Song(SongCommand::AddTrack { device: key(name) }))
            .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("a"),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    c.sink_mut().take();
    c
}

fn plays(c: &mut AppCore<RecordingSink>) -> Vec<(u32, NoteKind)> {
    c.sink_mut()
        .take()
        .into_iter()
        .filter_map(|call| match call {
            SinkCall::PlayNote { target, kind } => Some((target.0.0, kind)),
            _ => None,
        })
        .collect()
}

fn key_note(c: &mut AppCore<RecordingSink>, key: u8, velocity: u8, on: bool) {
    c.handle_action(UiAction::KeysNote { key, velocity, on });
}

#[test]
fn a_key_plays_the_armed_track_on_the_play_path_and_not_as_an_audition() {
    let mut c = with_armed_track();
    key_note(&mut c, 60, 127, true);
    key_note(&mut c, 60, 0, false);
    let calls = c.sink_mut().take();
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, SinkCall::LiveNote { .. }))
    );
    let played: Vec<_> = calls
        .into_iter()
        .filter_map(|call| match call {
            SinkCall::PlayNote { target, kind } => Some((target.0.0, kind)),
            _ => None,
        })
        .collect();
    assert_eq!(
        played,
        vec![
            (
                1,
                NoteKind::On {
                    key: 60,
                    velocity: 1.0
                }
            ),
            (1, NoteKind::Off { key: 60 }),
        ]
    );
}

#[test]
fn a_note_off_goes_where_its_note_on_went_when_the_target_moves() {
    let mut c = with_armed_track();
    key_note(&mut c, 62, 100, true);
    // Arming the other track moves the target while the key is held.
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("a"),
        muted: false,
        solo: false,
        armed: false,
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("b"),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    key_note(&mut c, 62, 0, false);
    key_note(&mut c, 64, 100, true);
    let played = plays(&mut c);
    let devices: Vec<u32> = played.iter().map(|(d, _)| *d).collect();
    assert_eq!(devices, vec![1, 1, 2], "{played:?}");
    assert!(matches!(played[1].1, NoteKind::Off { key: 62 }));
}

fn take_notes(c: &AppCore<RecordingSink>) -> Vec<Note> {
    let clip = &c.doc().song.tracks[0].lanes[0].clips[0];
    let ClipContent::Notes(notes) = &clip.content else {
        panic!("note clip");
    };
    notes.clone()
}

fn record_with(play: impl FnOnce(&mut AppCore<RecordingSink>, &dyn Fn(i64) -> u64)) -> Vec<Note> {
    let mut c = with_armed_track();
    c.handle_action(UiAction::Transport(TransportAction::ToggleRecord));
    let start = TransportState {
        playing: true,
        engine_frame: 1000,
        ..TransportState::default()
    };
    c.sink_mut().state = start;
    let frame = |tick: i64| 1000 + (tick as f64 * 1.5625).round() as u64;
    play(&mut c, &frame);
    c.frame();
    c.sink_mut().state = TransportState {
        tick: 2 * PPQ * 4,
        engine_frame: frame(2 * PPQ * 4),
        song_frame: frame(2 * PPQ * 4) - 1000,
        ..start
    };
    c.handle_action(UiAction::Transport(TransportAction::Stop));
    take_notes(&c)
}

/// What the strip records is what the same notes from a MIDI keyboard record: the sink hands the
/// recorder the notes it played, as the MIDI input does with its own.
#[test]
fn a_take_with_the_strip_is_the_take_a_keyboard_makes() {
    let keyboard = record_with(|c, frame| {
        c.sink_mut().recorded = vec![
            RecordedNote {
                device: DeviceInstanceId(1),
                kind: NoteKind::On {
                    key: 60,
                    velocity: 100.0 / 127.0,
                },
                frame: frame(PPQ),
            },
            RecordedNote {
                device: DeviceInstanceId(1),
                kind: NoteKind::Off { key: 60 },
                frame: frame(2 * PPQ),
            },
            RecordedNote {
                device: DeviceInstanceId(1),
                kind: NoteKind::On {
                    key: 67,
                    velocity: 50.0 / 127.0,
                },
                frame: frame(3 * PPQ),
            },
            RecordedNote {
                device: DeviceInstanceId(1),
                kind: NoteKind::Off { key: 67 },
                frame: frame(4 * PPQ),
            },
        ];
    });
    let strip = record_with(|c, frame| {
        for (tick, k, v, on) in [
            (PPQ, 60, 100, true),
            (2 * PPQ, 60, 0, false),
            (3 * PPQ, 67, 50, true),
            (4 * PPQ, 67, 0, false),
        ] {
            c.sink_mut().live_frame = frame(tick);
            key_note(c, k, v, on);
        }
    });
    assert_eq!(keyboard.len(), 2);
    assert_eq!(strip, keyboard);
}

#[test]
fn a_track_that_is_not_armed_records_nothing_from_the_strip() {
    let mut c = with_armed_track();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("a"),
        muted: false,
        solo: false,
        armed: false,
    }))
    .unwrap();
    key_note(&mut c, 60, 100, true);
    key_note(&mut c, 60, 0, false);
    assert!(c.sink_mut().recorded.is_empty());
}
