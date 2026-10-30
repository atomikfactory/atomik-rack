//! The audio clip editor in the app core: opening and closing, exclusivity with the other
//! editors, and Clip to Loop Slicer as one undo step.

use super::*;
use crate::import_warp::WarpImports;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{WarpMarker, empty_doc};
use atomik_ui::{AudioMode, ImportTarget};

fn core_with_clip() -> (AppCore<RecordingSink>, ClipId, PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("atomik-app-clipeditor-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loop.wav");
    let data: Vec<f32> = (0..96_000)
        .map(|i| ((i % 100) as f32 - 50.0) / 100.0)
        .collect();
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.set_warp_imports(WarpImports::Off);
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    (c, id, dir)
}

#[test]
fn opening_replaces_the_other_editors_and_closing_shows_the_arrangement() {
    let (mut c, id, dir) = core_with_clip();
    c.handle_action(UiAction::OpenAudioEditor(id));
    assert_eq!(c.audio_editor_clip(), Some(id));
    assert!(c.audio_editor().is_present());
    assert_eq!(c.arrangement().selected_clips(), vec![id]);
    c.handle_action(UiAction::CloseAudioEditor);
    assert_eq!(c.audio_editor_clip(), None);
    // A note clip or an unknown clip does not open it.
    c.handle_action(UiAction::OpenAudioEditor(ClipId(999)));
    assert_eq!(c.audio_editor_clip(), None);
    assert!(c.open_audio_editor_selected());
    assert!(c.close_open_editor());
    assert_eq!(c.audio_editor_clip(), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_removed_clip_closes_the_editor() {
    let (mut c, id, dir) = core_with_clip();
    c.handle_action(UiAction::OpenAudioEditor(id));
    c.apply_command(Command::Song(SongCommand::RemoveClip { id }))
        .unwrap();
    c.after_view_change();
    assert_eq!(c.audio_editor_clip(), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clip_to_slicer_is_one_step_and_slices_at_the_markers() {
    let (mut c, id, dir) = core_with_clip();
    // Without markers it is refused.
    c.clip_to_slicer(id);
    assert!(c.last_rejection().is_some());
    let depth = c.document().undo_depth();
    let clip = c.doc().song.clip(id).unwrap().2.clone();
    let ClipContent::Audio(a) = &clip.content else {
        panic!("audio")
    };
    let mut content = a.clone();
    content.warp.enabled = true;
    content.warp.source_bpm = 120.0;
    let spt = 60.0 * 48_000.0 / (120.0 * atomik_core::PPQ as f64);
    let at = |t: i64| WarpMarker {
        tick: t,
        source: (t as f64 * spt).round() as u64,
    };
    content.warp.markers = vec![at(0), at(10_000), at(20_000)];
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id,
        start: clip.start,
        length: clip.length,
        muted: clip.muted,
        content,
        merge: None,
    }))
    .unwrap();
    let depth_marked = c.document().undo_depth();
    assert_eq!(depth_marked, depth + 1);
    c.clip_to_slicer(id);
    assert_eq!(c.document().undo_depth(), depth_marked + 1, "one undo step");
    let slicer = c
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == "atomik.loop-slicer")
        .expect("a slicer");
    assert_eq!(slicer.zones.len(), 3, "{:?}", slicer.zones);
    assert_eq!(slicer.zones[1].start, at(10_000).source);
    c.undo();
    assert!(
        c.doc()
            .devices
            .iter()
            .all(|d| d.type_id != "atomik.loop-slicer")
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------ transients

/// Silence with three noise bursts, at frames 10 000, 40 000 and 70 000.
const BURSTS: [u64; 3] = [10_000, 40_000, 70_000];

fn core_with_bursts() -> (AppCore<RecordingSink>, ClipId, PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("atomik-app-onsets-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bursts.wav");
    let mut data = vec![0.0f32; 96_000];
    let mut seed = 12_345u32;
    for start in BURSTS {
        for i in 0..2_000usize {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            data[start as usize + i] = noise * 0.9 * (1.0 - i as f32 / 2_000.0);
        }
    }
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.set_warp_imports(WarpImports::Off);
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    (c, id, dir)
}

fn near_the_bursts(found: &[u64], bursts: &[u64]) {
    assert_eq!(found.len(), bursts.len(), "{found:?} vs {bursts:?}");
    for (f, b) in found.iter().zip(bursts) {
        assert!(f.abs_diff(*b) <= 2_048, "{f} is not near {b}: {found:?}");
    }
}

#[test]
fn opening_in_slice_mode_finds_the_transients_off_the_ui_thread() {
    let (mut c, id, dir) = core_with_bursts();
    c.audio_editor_mut().set_mode(AudioMode::Slice);
    c.handle_action(UiAction::OpenAudioEditor(id));
    assert!(c.audio_editor().transients().is_empty(), "not found yet");
    c.wait_background();
    near_the_bursts(c.audio_editor().transients(), &BURSTS);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn overview_mode_looks_for_nothing_until_slice_asks() {
    let (mut c, id, dir) = core_with_bursts();
    c.handle_action(UiAction::OpenAudioEditor(id));
    assert_eq!(c.audio.pending, 0);
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 50,
    });
    c.wait_background();
    near_the_bursts(c.audio_editor().transients(), &BURSTS);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_second_request_at_the_same_sensitivity_is_answered_from_the_cache() {
    let (mut c, id, dir) = core_with_bursts();
    c.handle_action(UiAction::OpenAudioEditor(id));
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 50,
    });
    c.wait_background();
    c.audio_editor_mut().set_transients(Vec::new());
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 50,
    });
    assert_eq!(c.audio.pending, 0, "no worker started");
    near_the_bursts(c.audio_editor().transients(), &BURSTS);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_the_newest_request_is_shown() {
    let (mut c, id, dir) = core_with_bursts();
    c.handle_action(UiAction::OpenAudioEditor(id));
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 20,
    });
    // The view moves on to another sensitivity before the first answer arrives.
    c.audio_editor_mut().set_sensitivity(90);
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 90,
    });
    c.wait_background();
    let high = c.audio_editor().transients().to_vec();
    assert!(!high.is_empty());
    // The slower request cached its own answer: asking for it again is instant and the editor,
    // now at that sensitivity, shows it.
    c.audio_editor_mut().set_sensitivity(20);
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 20,
    });
    near_the_bursts(c.audio_editor().transients(), &BURSTS);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_trimmed_clip_only_gets_the_transients_it_plays() {
    let (mut c, id, dir) = core_with_bursts();
    let clip = c.doc().song.clip(id).unwrap().2.clone();
    let ClipContent::Audio(a) = &clip.content else {
        panic!("audio")
    };
    let mut content = a.clone();
    content.source_offset = 30_000;
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id,
        start: clip.start,
        length: clip.length,
        muted: clip.muted,
        content,
        merge: None,
    }))
    .unwrap();
    c.handle_action(UiAction::OpenAudioEditor(id));
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 50,
    });
    c.wait_background();
    near_the_bursts(c.audio_editor().transients(), &BURSTS[1..]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_clip_that_is_not_open_or_not_audio_is_ignored() {
    let (mut c, id, dir) = core_with_bursts();
    c.handle_action(UiAction::DetectTransients {
        clip: ClipId(999),
        sensitivity: 50,
    });
    assert_eq!(c.audio.pending, 0);
    // A result for a clip the editor has since left is cached but not shown.
    c.handle_action(UiAction::OpenAudioEditor(id));
    c.handle_action(UiAction::DetectTransients {
        clip: id,
        sensitivity: 50,
    });
    c.handle_action(UiAction::CloseAudioEditor);
    c.wait_background();
    assert!(c.audio_editor().transients().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
