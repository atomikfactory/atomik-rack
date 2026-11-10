//! Things dropped on the launcher and dragged out of it: an arrangement clip into a slot, a slot
//! onto the arrangement, an audio file into the slot of an audio track and a MIDI file into the
//! slot of a note track. Each is one undo step.

use super::tests::rack_only_core;
use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, LauncherCommand, Note, SongCommand,
};
use atomik_ui::{LauncherAction, UiAction};
use std::path::PathBuf;

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key("a")),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
        .unwrap();
    c
}

fn arrangement_clip(c: &mut AppCore<RecordingSink>) -> ClipId {
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 2 * BAR,
            length: BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(vec![Note::new(0, PPQ, 60, 0.8)].into()),
        },
    }))
    .unwrap();
    c.document().last_clip_id().unwrap()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-slots-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_arrangement_clip_dragged_onto_a_slot_is_copied_into_it_in_one_step() {
    let mut c = core();
    let id = arrangement_clip(&mut c);
    c.set_launcher_visible(true);
    c.launcher_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 760.0, 420.0));
    c.sync_launcher();
    let at = {
        let r = c.launcher().slot_rect(0, 2).expect("the slot shows");
        r.center()
    };
    let depth = c.document().undo_depth();
    c.handle_action(UiAction::ClipsDragged {
        ids: vec![id],
        pos: at,
    });
    assert_eq!(c.document().undo_depth(), depth + 1);
    let slot = c.doc().song.launcher.slot(&key("a"), 2).expect("copied");
    assert_ne!(slot.clip.id, id);
    assert!(c.doc().song.clip(id).is_some(), "the arrangement keeps it");
    c.undo();
    assert!(c.doc().song.launcher.slot(&key("a"), 2).is_none());
    // Dropped away from every slot: nothing happens.
    c.handle_action(UiAction::ClipsDragged {
        ids: vec![id],
        pos: atomik_ui::kurbo::Point::new(5_000.0, 5_000.0),
    });
    assert!(c.doc().song.launcher.slot(&key("a"), 2).is_none());
}

#[test]
fn a_slot_dragged_out_of_the_launcher_lands_on_the_arrangement_lane_it_is_dropped_on() {
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::Launcher(
        LauncherCommand::NewClip {
            device: key("a"),
            row: 0,
            length: BAR,
        },
    )))
    .unwrap();
    // A drop where the arrangement has no lane does nothing; the command itself is covered in
    // the model tests (a slot goes to a track at a tick).
    let depth = c.document().undo_depth();
    c.handle_action(UiAction::Launcher(LauncherAction::DragOut {
        device: key("a"),
        row: 0,
        pos: atomik_ui::kurbo::Point::new(-10.0, -10.0),
    }));
    assert_eq!(c.document().undo_depth(), depth);
    // Dropped on the arrangement's lane of the track: a clip there, one undo step.
    c.arrangement_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 450.0, 1200.0, 800.0));
    c.after_view_change();
    let pos = (450..800)
        .step_by(3)
        .map(|y| atomik_ui::kurbo::Point::new(700.0, f64::from(y)))
        .find(|p| {
            c.arrangement()
                .drop_spot_at(*p)
                .is_some_and(|(lane, _)| lane.is_some())
        })
        .expect("a lane row of the arrangement");
    c.handle_action(UiAction::Launcher(LauncherAction::DragOut {
        device: key("a"),
        row: 0,
        pos,
    }));
    assert_eq!(c.document().undo_depth(), depth + 1);
    let lane = &c.doc().song.track(&key("a")).unwrap().lanes[0];
    assert_eq!(lane.clips.len(), 1);
    assert_eq!(lane.clips[0].length, BAR);
    c.undo();
    assert!(
        c.doc().song.track(&key("a")).unwrap().lanes[0]
            .clips
            .is_empty()
    );
}

#[test]
fn a_midi_file_dropped_on_a_note_slot_becomes_its_clip_and_a_wrong_file_changes_nothing() {
    let dir = temp_dir("midi");
    let mut source = core();
    let id = arrangement_clip(&mut source);
    let file = dir.join("riff.mid");
    let bytes = {
        let song = &source.doc().song;
        let clip = &song.clip(id).unwrap().2;
        crate::midi_file::clip_file(
            song,
            clip,
            "riff",
            crate::midi_file::MidiResolution::default(),
        )
    };
    std::fs::write(&file, bytes).unwrap();
    let mut c = core();
    let depth = c.document().undo_depth();
    c.handle_action(UiAction::Launcher(LauncherAction::DropFile {
        device: key("a"),
        row: 1,
        path: file,
    }));
    assert_eq!(c.document().undo_depth(), depth + 1);
    let slot = c.doc().song.launcher.slot(&key("a"), 1).expect("the clip");
    assert!(matches!(&slot.clip.content, ClipContent::Notes(n) if n.len() == 1));
    // An audio file on a note slot is refused with a message.
    let wav = dir.join("tone.wav");
    atomik_audio_file::write_wav_f32(
        &wav,
        &atomik_audio_file::AudioData::new(48_000, vec![vec![0.1; 4_800]]).unwrap(),
    )
    .unwrap();
    c.handle_action(UiAction::Launcher(LauncherAction::DropFile {
        device: key("a"),
        row: 2,
        path: wav,
    }));
    c.wait_background();
    assert!(c.doc().song.launcher.slot(&key("a"), 2).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_audio_file_dropped_on_an_audio_slot_becomes_its_clip_and_keeps_its_file() {
    let dir = temp_dir("audio");
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::AddAudioTrack { name: None }))
        .unwrap();
    let audio = c
        .doc()
        .song
        .tracks
        .iter()
        .find(|t| t.is_audio())
        .unwrap()
        .device
        .clone();
    let wav = dir.join("tone.wav");
    atomik_audio_file::write_wav_f32(
        &wav,
        &atomik_audio_file::AudioData::new(48_000, vec![vec![0.1; 48_000]]).unwrap(),
    )
    .unwrap();
    c.handle_action(UiAction::Launcher(LauncherAction::DropFile {
        device: audio.clone(),
        row: 0,
        path: wav,
    }));
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let slot = c.doc().song.launcher.slot(&audio, 0).expect("the clip");
    let ClipContent::Audio(a) = &slot.clip.content else {
        panic!("not audio");
    };
    assert!(c.doc().assets.contains_key(&a.asset));
    assert!(c.doc().referenced_assets().contains(&a.asset));
    // One second at 120 BPM is two beats.
    assert_eq!(slot.clip.length, 2 * PPQ);
    c.undo();
    assert!(c.doc().song.launcher.slot(&audio, 0).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn record_launches_writes_an_audio_slot_as_a_looped_audio_clip() {
    let dir = temp_dir("rec");
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::AddAudioTrack { name: None }))
        .unwrap();
    let audio = c
        .doc()
        .song
        .tracks
        .iter()
        .find(|t| t.is_audio())
        .unwrap()
        .device
        .clone();
    let wav = dir.join("tone.wav");
    atomik_audio_file::write_wav_f32(
        &wav,
        &atomik_audio_file::AudioData::new(48_000, vec![vec![0.1; 48_000]]).unwrap(),
    )
    .unwrap();
    c.handle_action(UiAction::Launcher(LauncherAction::DropFile {
        device: audio.clone(),
        row: 0,
        path: wav,
    }));
    c.wait_background();
    // The slot is two beats long; launched at bar 2 and stopped at bar 3 it plays 2 bars.
    c.set_record_launches(true);
    c.transport_action(atomik_ui::TransportAction::ToggleRecord);
    let device = c
        .device_id(&audio)
        .expect("the audio track has an engine id");
    c.sink_mut()
        .launch_events
        .push_back(atomik_engine::LaunchEvent {
            device,
            slot: Some(0),
            tick: BAR,
        });
    c.sink_mut().state.playing = true;
    c.sink_mut().state.tick = 3 * BAR;
    let depth = c.document().undo_depth();
    c.transport_action(atomik_ui::TransportAction::Stop);
    assert_eq!(c.document().undo_depth(), depth + 1);
    let lane = &c.doc().song.track(&audio).unwrap().lanes[0];
    assert_eq!(lane.clips.len(), 1);
    let k = &lane.clips[0];
    assert!(k.content.is_audio());
    assert_eq!(
        (k.start, k.length, k.loop_length),
        (BAR, 2 * BAR, Some(2 * PPQ))
    );
    let _ = std::fs::remove_dir_all(&dir);
}
