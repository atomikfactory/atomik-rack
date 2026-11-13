//! Converting the loop of an audio clip to one clip: a render on the freeze worker, then one
//! `RenderLoop` step.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{ClipId, Command, LoopConvert, SongCommand, WorkflowCommand, new_project_doc};
use atomik_ui::ImportTarget;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

fn scratch_wav(tag: &str, frames: usize) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("atomik-app-loops-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..frames)
        .map(|i| 0.5 * i as f32 / frames as f32)
        .collect();
    let path = dir.join("loop.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

/// A project with one audio track holding a one-bar clip looped over three bars.
fn looped_core() -> (AppCore<RecordingSink>, ClipId, PathBuf) {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    let wav = scratch_wav("convert", 96_000);
    c.import_audio(vec![wav.clone()], ImportTarget::NewTracks, 0);
    c.wait_background();
    let track = c.doc().song.tracks[0].device.clone();
    let clip = c.doc().song.tracks[0].lanes[0].clips[0].clone();
    assert_eq!(clip.length, BAR, "one bar of audio at 120 BPM");
    c.apply_command(Command::Song(SongCommand::SetClipGeometry {
        id: clip.id,
        start: 0,
        length: 3 * BAR,
        offset: 0,
        loop_length: Some(BAR),
        muted: false,
        merge: None,
    }))
    .unwrap();
    assert_eq!(c.doc().song.track(&track).unwrap().lanes[0].clips.len(), 1);
    (c, clip.id, wav)
}

fn convert(c: &mut AppCore<RecordingSink>, ids: Vec<ClipId>) {
    c.apply_command(Command::Workflow(WorkflowCommand::ConvertLoop {
        ids,
        mode: LoopConvert::Single,
    }))
    .unwrap();
    c.wait_background();
}

#[test]
fn an_audio_loop_converts_to_one_clip_of_its_render_in_one_undo_step() {
    let (mut c, id, wav) = looped_core();
    let depth = c.document().undo_depth();
    convert(&mut c, vec![id]);
    let track = &c.doc().song.tracks[0];
    assert_eq!(track.lanes[0].clips.len(), 1);
    let clip = &track.lanes[0].clips[0];
    assert_eq!((clip.id, clip.start, clip.length), (id, 0, 3 * BAR));
    assert!(clip.loop_length.is_none() && clip.offset == 0);
    let ClipContent::Audio(a) = &clip.content else {
        panic!("an audio clip");
    };
    let meta = &c.doc().assets[&a.asset];
    assert_eq!(
        meta.frames,
        3 * 96_000,
        "the render holds the three repetitions"
    );
    assert_eq!(a.source_offset, 0);
    assert_eq!(c.arrangement().selected_clips(), vec![id]);
    // The render and the clip arrive in one step on top of the registered asset.
    assert!(c.document().undo_depth() <= depth + 2);
    c.undo();
    let back = &c.doc().song.tracks[0].lanes[0].clips[0];
    assert_eq!((back.loop_length, back.length), (Some(BAR), 3 * BAR));
    let _ = std::fs::remove_dir_all(wav.parent().unwrap());
}

#[test]
fn an_unknown_clip_queues_no_render() {
    let (mut c, _, wav) = looped_core();
    let plain = ClipId(9999);
    // An unknown clip falls through to the model, which says there is no such clip.
    let r = c.apply_command(Command::Workflow(WorkflowCommand::ConvertLoop {
        ids: vec![plain],
        mode: LoopConvert::Single,
    }));
    assert!(r.is_err());
    assert!(!c.freeze_busy(), "nothing was queued");
    let _ = std::fs::remove_dir_all(wav.parent().unwrap());
}

fn lane_clip(key: u8) -> atomik_model::Clip {
    atomik_model::Clip {
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
        content: ClipContent::Notes(vec![atomik_model::Note::new(0, PPQ, key, 0.8)].into()),
    }
}

#[test]
fn a_lane_solo_changes_the_notes_the_engine_gets() {
    use crate::sink::SinkCall;
    use atomik_model::{CreateOptions, CreateSource, DeviceKey};
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    let lead = DeviceKey("lead".to_owned());
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(lead.clone()),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddNoteLane {
        device: lead.clone(),
        name: None,
    }))
    .unwrap();
    for (lane, key) in [(0, 60), (1, 67)] {
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: lead.clone(),
            lane,
            clip: lane_clip(key),
        }))
        .unwrap();
    }
    let last_keys = |c: &mut AppCore<RecordingSink>| -> Vec<u8> {
        let calls = c.sink_mut().take();
        calls
            .iter()
            .rev()
            .find_map(|call| match call {
                SinkCall::Song(input) => Some(
                    input
                        .tracks
                        .iter()
                        .flat_map(|t| t.notes.iter().map(|n| n.key))
                        .collect(),
                ),
                _ => None,
            })
            .expect("the song was handed to the engine")
    };
    assert_eq!(last_keys(&mut c), vec![60, 67]);
    c.apply_command(Command::Song(SongCommand::SetLaneSolo {
        device: lead.clone(),
        lane: 1,
        solo: true,
    }))
    .unwrap();
    assert_eq!(last_keys(&mut c), vec![67]);
    c.undo();
    assert_eq!(last_keys(&mut c), vec![60, 67]);
}
