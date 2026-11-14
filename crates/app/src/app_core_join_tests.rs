//! Joining audio clips of different files: a render on the freeze worker, then one `RenderJoin`
//! step.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_engine::SongAsset;
use atomik_model::{ClipId, Command, SongCommand, WorkflowCommand, new_project_doc};
use atomik_ui::ImportTarget;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

fn scratch_wav(dir: &std::path::Path, name: &str, frames: usize, level: f32) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .map(|i| level * i as f32 / frames as f32)
        .collect();
    let path = dir.join(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

/// A project with one audio track holding a one-bar clip of one file and, right after it, a
/// half-bar clip of another.
fn two_files() -> (AppCore<RecordingSink>, Vec<ClipId>, PathBuf) {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("atomik-app-join-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    let first = scratch_wav(&dir, "one.wav", 96_000, 0.5);
    let second = scratch_wav(&dir, "two.wav", 48_000, 0.25);
    c.import_audio(vec![first], ImportTarget::NewTracks, 0);
    c.wait_background();
    let track = c.doc().song.tracks[0].device.clone();
    c.import_audio(vec![second], ImportTarget::ExistingTrack(track), BAR);
    c.wait_background();
    let clips = &c.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 2);
    let ids = clips.iter().map(|k| k.id).collect();
    (c, ids, dir)
}

fn join(c: &mut AppCore<RecordingSink>, ids: Vec<ClipId>) {
    c.apply_command(Command::Workflow(WorkflowCommand::JoinClips { ids }))
        .unwrap();
    c.wait_background();
}

#[test]
fn clips_of_different_files_join_into_one_clip_of_their_render_in_one_undo_step() {
    let (mut c, ids, dir) = two_files();
    let before = c.doc().song.tracks.clone();
    let depth = c.document().undo_depth();
    join(&mut c, ids.clone());
    let clips = &c.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 1, "one clip now");
    let clip = &clips[0];
    assert_eq!(clip.id, ids[0], "the earliest clip keeps its id");
    assert_eq!((clip.start, clip.length), (0, BAR + BAR / 2));
    let ClipContent::Audio(a) = &clip.content else {
        panic!("an audio clip");
    };
    assert_eq!(a.source_offset, 0);
    assert!(a.warp.markers.is_empty() && !a.reverse);
    let meta = &c.doc().assets[&a.asset];
    assert_eq!(meta.frames, 96_000 + 48_000, "the render covers both clips");
    assert_eq!(c.arrangement().selected_clips(), vec![ids[0]]);
    // The asset and the join arrive in two steps at most: one undo gives the clips back.
    assert!(c.document().undo_depth() <= depth + 2);
    c.undo();
    assert_eq!(c.doc().song.tracks, before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_render_runs_off_the_ui_thread() {
    let (mut c, ids, dir) = two_files();
    let before = c.doc().song.tracks.clone();
    c.apply_command(Command::Workflow(WorkflowCommand::JoinClips { ids }))
        .unwrap();
    // The command returned with the render queued or running: the document is not changed yet.
    assert!(c.freeze_busy());
    assert_eq!(c.doc().song.tracks, before);
    c.wait_background();
    assert!(!c.freeze_busy());
    assert_eq!(c.doc().song.tracks[0].lanes[0].clips.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_gap_refuses_without_a_render() {
    let (mut c, ids, dir) = two_files();
    c.apply_command(Command::Song(SongCommand::SetClipGeometry {
        id: ids[1],
        start: BAR + PPQ,
        length: BAR / 2,
        offset: 0,
        loop_length: None,
        muted: false,
        merge: None,
    }))
    .unwrap();
    let before = c.doc().song.tracks.clone();
    join(&mut c, ids.clone());
    assert!(!c.freeze_busy(), "nothing was queued");
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("does not touch")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(c.doc().song.tracks, before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clips_of_one_file_still_join_without_a_render() {
    let (mut c, ids, dir) = two_files();
    // Two pieces of the first file.
    let first = c.doc().song.tracks[0].lanes[0].clips[0].clone();
    let ClipContent::Audio(a) = &first.content else {
        panic!("audio");
    };
    let asset = a.asset.clone();
    c.apply_command(Command::Song(SongCommand::RemoveClip { id: ids[1] }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::SplitClip {
        id: ids[0],
        at: BAR / 2,
    }))
    .unwrap();
    let ids: Vec<ClipId> = c.doc().song.tracks[0].lanes[0]
        .clips
        .iter()
        .map(|k| k.id)
        .collect();
    c.apply_command(Command::Workflow(WorkflowCommand::JoinClips { ids }))
        .unwrap();
    assert!(!c.freeze_busy(), "a join of one file renders nothing");
    let clips = &c.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 1);
    let ClipContent::Audio(joined) = &clips[0].content else {
        panic!("audio");
    };
    assert_eq!(joined.asset, asset, "still the original file");
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------ Flatten Comp

/// Two takes of one bar over the same bar on one track: the first file ramps up to 0.5, the
/// second to 0.25. Take 0 plays the first half of the bar, take 1 the second half.
fn comped() -> (AppCore<RecordingSink>, u32, PathBuf) {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("atomik-app-flatten-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.import_audio(
        vec![scratch_wav(&dir, "one.wav", 96_000, 0.5)],
        ImportTarget::NewTracks,
        0,
    );
    c.wait_background();
    let track = c.doc().song.tracks[0].device.clone();
    c.import_audio(
        vec![scratch_wav(&dir, "two.wav", 96_000, 0.25)],
        ImportTarget::ExistingTrack(track.clone()),
        0,
    );
    c.wait_background();
    let ids: Vec<ClipId> = c.doc().song.tracks[0].lanes[0]
        .clips
        .iter()
        .map(|k| k.id)
        .collect();
    assert_eq!(ids.len(), 2);
    c.apply_command(Command::Song(SongCommand::GroupAsTakes {
        device: track,
        lane: 0,
        clips: ids,
        extra: Vec::new(),
    }))
    .unwrap();
    let group = c.doc().song.take_groups().next().unwrap().2.id;
    c.apply_command(Command::Song(SongCommand::SetCompSegment {
        group,
        take: 0,
        start: 0,
        end: BAR / 2,
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetCompSegment {
        group,
        take: 1,
        start: BAR / 2,
        end: BAR,
    }))
    .unwrap();
    (c, group, dir)
}

#[test]
fn flatten_comp_renders_the_comp_to_one_file_in_one_undo_step() {
    let (mut c, group, dir) = comped();
    let before = c.doc().song.tracks.clone();
    c.flatten_comp(group);
    // The render runs on the freeze worker: nothing changes until it is done.
    assert!(c.freeze_busy());
    assert_eq!(c.doc().song.tracks, before);
    c.wait_background();
    assert!(
        c.doc().song.take_groups().next().is_none(),
        "the group is gone"
    );
    let clips = &c.doc().song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 1);
    let clip = &clips[0];
    assert_eq!((clip.start, clip.length), (0, BAR));
    let ClipContent::Audio(a) = &clip.content else {
        panic!("an audio clip");
    };
    assert_eq!(
        c.doc().assets[&a.asset].frames,
        96_000,
        "the render covers the comp"
    );
    // The file plays what the comp played: the first file's ramp in the first half, the second
    // file's in the second, each scaled by the same track gain.
    let SongAsset::Memory { data, .. } = &c.audio.assets[&a.asset] else {
        panic!("the render is in memory");
    };
    let ramp = |level: f32, i: usize| level * i as f32 / 96_000.0;
    let k0 = data[0][20_000] / ramp(0.5, 20_000);
    let k1 = data[0][70_000] / ramp(0.25, 70_000);
    assert!(k0 > 0.1, "{k0}");
    assert!((k0 - k1).abs() < 1.0e-3, "{k0} against {k1}");
    // One undo brings the takes and the comp back.
    c.undo();
    assert_eq!(c.doc().song.tracks, before);
    assert_eq!(c.doc().song.take_groups().count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flatten_comp_refuses_an_empty_comp_and_a_gone_group() {
    let (mut c, group, dir) = comped();
    c.flatten_comp(group + 40);
    assert!(!c.freeze_busy());
    assert!(c.last_rejection().is_some_and(|m| m.contains("gone")));
    c.apply_command(Command::Song(SongCommand::SetCompSilence {
        group,
        start: 0,
        end: BAR,
    }))
    .unwrap();
    c.flatten_comp(group);
    assert!(!c.freeze_busy());
    assert!(c.last_rejection().is_some_and(|m| m.contains("nothing")));
    let _ = std::fs::remove_dir_all(dir);
}
