//! Stem separation in the app core, with a fake backend: no model, no network. The worker reads a
//! generated file from the project's bundle, the fake separates it into scaled copies, the stems
//! are stored and one undo step adds the tracks.

use super::*;
use crate::sink::RecordingSink;
use crate::stems_link::FakeStems;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::empty_doc;
use atomik_ui::{ImportTarget, StemsDialogChoice as Choice, StemsDialogState as State};
use std::sync::atomic::{AtomicU64, Ordering};

const SR: u32 = 48_000;
/// The fake runs at another rate, so the resampling there and back is part of every test.
const FAKE_RATE: u32 = 24_000;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-stems-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core(fake: &FakeStems) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.set_warp_imports(crate::import_warp::WarpImports::Off);
    c.set_stems(Arc::new(fake.clone()));
    c.sink_mut().take();
    c
}

/// A slow sine, `seconds` long.
fn sine(seconds: f64) -> Vec<f32> {
    (0..(seconds * f64::from(SR)) as usize)
        .map(|i| 0.5 * (i as f32 * 0.02).sin())
        .collect()
}

/// Imports a generated file at tick `at` on a new track and selects its clip.
fn import(c: &mut AppCore<RecordingSink>, dir: &Tmp, channels: Vec<Vec<f32>>, at: i64) -> ClipId {
    let path = dir.0.join("song.wav");
    write_wav_f32(&path, &AudioData::new(SR, channels).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, at);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let id = c.doc().song.tracks.last().unwrap().lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    id
}

fn stereo(c: &mut AppCore<RecordingSink>, dir: &Tmp, at: i64) -> ClipId {
    let x = sine(2.0);
    import(c, dir, vec![x.clone(), x], at)
}

/// Opens the dialog and runs the separation with the models ready.
fn separate(c: &mut AppCore<RecordingSink>) {
    c.separate_stems().expect("the dialog opens");
    c.stems_dialog_choice(Choice::Separate);
    c.wait_background();
}

fn track_count(c: &AppCore<RecordingSink>) -> usize {
    c.doc().song.tracks.len()
}

#[test]
fn the_command_makes_a_track_per_stem_with_the_clip_at_the_same_place() {
    let dir = Tmp::new("tracks");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    let at = atomik_core::PPQ * 4;
    let id = stereo(&mut c, &dir, at);
    let before = track_count(&c);
    let (start, length, offset) = {
        let (_, _, clip) = c.doc().song.clip(id).unwrap();
        let ClipContent::Audio(a) = &clip.content else {
            panic!("audio")
        };
        (clip.start, clip.length, a.source_offset)
    };
    separate(&mut c);
    assert!(c.last_rejection().is_some_and(|m| m.contains("Separated")));
    assert!(c.stems_dialog().is_none(), "the dialog closed");
    assert_eq!(track_count(&c), before + 4);
    let names: Vec<String> = c.doc().song.tracks[before..]
        .iter()
        .map(|t| c.doc().device(&t.device).unwrap().name.clone())
        .collect();
    assert!(names[0].ends_with("Vocals") && names[1].ends_with("Drums"));
    assert!(names[2].ends_with("Bass") && names[3].ends_with("Other"));
    for track in &c.doc().song.tracks[before..] {
        assert!(track.is_audio());
        let clip = &track.lanes[0].clips[0];
        assert_eq!((clip.start, clip.length), (start, length));
        let ClipContent::Audio(a) = &clip.content else {
            panic!("audio")
        };
        assert_eq!(a.source_offset, offset);
        let meta = c.doc().assets.get(&a.asset).expect("registered");
        assert_eq!(meta.sample_rate, SR, "back at the file's own rate");
        assert_eq!(
            meta.frames,
            (2.0 * f64::from(SR)) as u64,
            "as long as the file"
        );
    }
    assert_eq!(fake.separations(), 1);
}

#[test]
fn the_stem_files_are_in_the_bundle_and_carry_the_source_scaled() {
    let dir = Tmp::new("files");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    stereo(&mut c, &dir, 0);
    let before = track_count(&c);
    separate(&mut c);
    let bundle = c.import_bundle().unwrap();
    let mut peaks = Vec::new();
    for track in &c.doc().song.tracks[before..] {
        let ClipContent::Audio(a) = &track.lanes[0].clips[0].content else {
            panic!("audio")
        };
        assert!(bundle.exists(&a.asset), "the file is stored");
        let data = bundle.load(&a.asset).unwrap();
        peaks.push(data.channels[0].iter().fold(0f32, |m, v| m.max(v.abs())));
    }
    // The fake gives 0.1, 0.2, 0.3 and 0.4 of the source (peak 0.5), through two resamplings.
    for (i, p) in peaks.iter().enumerate() {
        let want = 0.5 * 0.1 * (i as f32 + 1.0);
        assert!((p - want).abs() < 0.02, "stem {i}: {p} vs {want}");
    }
}

#[test]
fn it_is_one_undo_step_and_undo_removes_everything() {
    let dir = Tmp::new("undo");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    let id = stereo(&mut c, &dir, 0);
    let before_doc = c.doc().clone();
    separate(&mut c);
    assert_eq!(c.doc().song.tracks.len(), before_doc.song.tracks.len() + 4);
    assert!(c.doc().song.clip(id).unwrap().2.muted);
    assert!(c.undo());
    assert_eq!(c.doc().song.tracks, before_doc.song.tracks);
    assert!(!c.doc().song.clip(id).unwrap().2.muted);
    assert!(c.redo());
    assert_eq!(c.doc().song.tracks.len(), before_doc.song.tracks.len() + 4);
}

#[test]
fn the_original_stays_unmuted_when_the_box_is_off_and_only_checked_stems_are_made() {
    let dir = Tmp::new("options");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    let id = stereo(&mut c, &dir, 0);
    let before = track_count(&c);
    c.separate_stems().unwrap();
    c.stems_dialog_choice(Choice::ToggleMute);
    c.stems_dialog_choice(Choice::Toggle(1));
    c.stems_dialog_choice(Choice::Toggle(3));
    let Some(State::Options {
        stems,
        mute_original,
        ..
    }) = c.stems_dialog()
    else {
        panic!("the options")
    };
    assert_eq!((stems, mute_original), ([true, false, true, false], false));
    c.stems_dialog_choice(Choice::Separate);
    c.wait_background();
    assert_eq!(track_count(&c), before + 2);
    assert!(!c.doc().song.clip(id).unwrap().2.muted);
}

#[test]
fn with_no_stem_checked_nothing_starts() {
    let dir = Tmp::new("none");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    stereo(&mut c, &dir, 0);
    c.separate_stems().unwrap();
    for i in 0..4 {
        c.stems_dialog_choice(Choice::Toggle(i));
    }
    c.stems_dialog_choice(Choice::Separate);
    assert!(matches!(c.stems_dialog(), Some(State::Options { .. })));
    assert!(!c.stems_running());
    assert_eq!(fake.separations(), 0);
}

#[test]
fn a_mono_file_gives_mono_stems() {
    let dir = Tmp::new("mono");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    import(&mut c, &dir, vec![sine(1.0)], 0);
    let before = track_count(&c);
    separate(&mut c);
    let ClipContent::Audio(a) = &c.doc().song.tracks[before].lanes[0].clips[0].content else {
        panic!("audio")
    };
    assert_eq!(c.doc().assets.get(&a.asset).unwrap().channels, 1);
}

#[test]
fn nothing_is_fetched_before_the_user_agrees_and_then_it_separates() {
    let dir = Tmp::new("consent");
    let fake = FakeStems::new(false, FAKE_RATE);
    let mut c = core(&fake);
    stereo(&mut c, &dir, 0);
    let before = track_count(&c);
    c.separate_stems().unwrap();
    c.stems_dialog_choice(Choice::Separate);
    match c.stems_dialog() {
        Some(State::Consent { source, bytes }) => {
            assert_eq!(source, "models.example");
            assert!(bytes > 100_000_000);
        }
        other => panic!("the consent question: {other:?}"),
    }
    c.wait_background();
    assert_eq!(fake.downloads(), 0, "nothing is fetched before Download");
    assert_eq!(fake.separations(), 0);
    assert_eq!(track_count(&c), before);
    // Cancel closes it and still nothing was fetched.
    c.stems_dialog_choice(Choice::Cancel);
    assert!(c.stems_dialog().is_none());
    assert_eq!(fake.downloads(), 0);
    // Ask again, agree: the download, then the separation.
    c.separate_stems().unwrap();
    c.stems_dialog_choice(Choice::Separate);
    c.stems_dialog_choice(Choice::Download);
    c.wait_background();
    assert_eq!(fake.downloads(), 1);
    assert_eq!(fake.separations(), 1);
    assert_eq!(track_count(&c), before + 4);
}

#[test]
fn a_failed_download_can_be_tried_again() {
    let dir = Tmp::new("failed");
    let fake = FakeStems::new(false, FAKE_RATE);
    fake.fail_download("no connection");
    let mut c = core(&fake);
    stereo(&mut c, &dir, 0);
    let before = track_count(&c);
    c.separate_stems().unwrap();
    c.stems_dialog_choice(Choice::Separate);
    c.stems_dialog_choice(Choice::Download);
    c.wait_background();
    match c.stems_dialog() {
        Some(State::Failed { message, retry }) => {
            assert!(message.contains("no connection"));
            assert!(retry);
        }
        other => panic!("the failure: {other:?}"),
    }
    assert_eq!(track_count(&c), before);
    c.stems_dialog_choice(Choice::Retry);
    c.wait_background();
    assert!(c.stems_dialog().is_none());
    assert_eq!(track_count(&c), before + 4);
}

#[test]
fn the_worker_cancels_cleanly_and_nothing_is_added() {
    let dir = Tmp::new("cancel");
    let fake = FakeStems::new(true, FAKE_RATE);
    fake.hold();
    let mut c = core(&fake);
    stereo(&mut c, &dir, 0);
    let before_doc = c.doc().clone();
    c.separate_stems().unwrap();
    c.stems_dialog_choice(Choice::Separate);
    assert!(c.stems_running());
    // Wait until the worker is inside the separation.
    for _ in 0..2000 {
        if fake.separations() == 1 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(fake.separations(), 1);
    assert!(matches!(c.stems_dialog(), Some(State::Working { .. })));
    c.stems_dialog_choice(Choice::Cancel);
    c.wait_background();
    assert!(!c.stems_running());
    assert!(c.stems_dialog().is_none());
    assert_eq!(c.doc().song.tracks, before_doc.song.tracks);
    assert!(c.last_rejection().is_some_and(|m| m.contains("cancelled")));
}

#[test]
fn the_dialog_needs_exactly_one_audio_clip() {
    let dir = Tmp::new("selection");
    let fake = FakeStems::new(true, FAKE_RATE);
    let mut c = core(&fake);
    let a = stereo(&mut c, &dir, 0);
    let b = stereo(&mut c, &dir, atomik_core::PPQ * 16);
    c.arrangement_mut().set_selected_clips(&[]);
    assert!(c.separate_stems().is_err());
    assert!(c.stems_dialog().is_none());
    c.arrangement_mut().set_selected_clips(&[a, b]);
    assert!(c.separate_stems().is_err());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("one audio clip"))
    );
    assert!(c.stems_dialog().is_none());
}

#[test]
fn a_clip_that_went_while_the_worker_ran_gets_nothing() {
    let dir = Tmp::new("gone");
    let fake = FakeStems::new(true, FAKE_RATE);
    fake.hold();
    let mut c = core(&fake);
    let id = stereo(&mut c, &dir, 0);
    c.separate_stems().unwrap();
    c.stems_dialog_choice(Choice::Separate);
    let _ = c.apply_command(Command::Song(SongCommand::RemoveClip { id }));
    let tracks = c.doc().song.tracks.clone();
    fake.release();
    c.wait_background();
    assert_eq!(c.doc().song.tracks, tracks);
    assert!(c.last_rejection().is_some_and(|m| m.contains("changed")));
}
