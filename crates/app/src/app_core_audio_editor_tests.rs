//! The audio clip editor in the app core: opening and closing, exclusivity with the other
//! editors, and Clip to Loop Slicer as one undo step.

use super::*;
use crate::import_warp::WarpImports;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{WarpMarker, empty_doc};
use atomik_ui::ImportTarget;

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
