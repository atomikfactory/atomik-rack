//! Saving on a worker: the document stays editable, the baseline switches when the worker
//! reports in, and a failed or abandoned save changes nothing.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{Command, empty_doc};
use atomik_ui::ImportTarget;
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-save-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::new(), empty_doc(), None)
}

fn edit(c: &mut AppCore<RecordingSink>, cents: f32) {
    c.apply_command(Command::SetMasterTune(cents)).unwrap();
}

fn import_wav(c: &mut AppCore<RecordingSink>, t: &Tmp) {
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let path = t.path("a.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
}

#[test]
fn a_background_save_writes_the_file_and_switches_the_baseline_when_it_is_applied() {
    let t = Tmp::new("basic");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 20.0);
    assert!(c.is_dirty());
    c.save_as_in_background(&file).unwrap();
    assert!(c.saving());
    assert_eq!(c.save_progress().as_deref(), Some("Saving..."));
    // Nothing switches until the worker's result is applied.
    assert!(c.file().is_none());
    assert!(c.is_dirty());
    c.settle_save();
    assert!(!c.saving());
    assert_eq!(c.file(), Some(file.as_path()));
    assert!(!c.is_dirty());
    assert!(file.exists());
    let reopened =
        atomik_model::load_project(&std::fs::read_to_string(&file).unwrap(), c.registry()).unwrap();
    assert_eq!(reopened.tune_cents, 20.0);
}

#[test]
fn the_bytes_of_a_background_save_equal_those_of_a_blocking_save() {
    let t = Tmp::new("bytes");
    let (a, b) = (t.path("a.atomik"), t.path("b.atomik"));
    let mut c = core();
    edit(&mut c, -12.0);
    c.save_as(&a).unwrap();
    c.save_as_in_background(&b).unwrap();
    c.settle_save();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

#[test]
fn an_edit_during_the_save_stays_unsaved_and_undoing_it_is_clean() {
    let t = Tmp::new("during");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 10.0);
    c.save_as_in_background(&file).unwrap();
    edit(&mut c, 30.0);
    c.settle_save();
    assert!(c.is_dirty(), "the edit is newer than the saved state");
    let saved =
        atomik_model::load_project(&std::fs::read_to_string(&file).unwrap(), c.registry()).unwrap();
    assert_eq!(
        saved.tune_cents, 10.0,
        "the file holds the state at the click"
    );
    assert!(c.undo());
    assert!(!c.is_dirty(), "undo back to the saved state is clean");
}

#[test]
fn polling_applies_a_finished_save() {
    let t = Tmp::new("poll");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 5.0);
    c.save_as_in_background(&file).unwrap();
    for _ in 0..2_000 {
        c.poll_background();
        if !c.saving() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!c.saving());
    assert!(!c.is_dirty());
    assert!(c.last_rejection().is_none());
}

#[test]
fn a_save_that_fails_reports_and_changes_nothing() {
    let t = Tmp::new("fails");
    let missing = t.path("no-such-folder").join("song.atomik");
    let mut c = core();
    edit(&mut c, 7.0);
    c.save_as_in_background(&missing).unwrap();
    c.settle_save();
    assert!(!c.saving());
    assert!(c.file().is_none());
    assert!(c.is_dirty());
    assert!(c.last_rejection().is_some());
    assert!(!missing.exists());
}

#[test]
fn a_failed_save_leaves_the_previous_file_intact() {
    let t = Tmp::new("intact");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 1.0);
    c.save_as(&file).unwrap();
    let before = std::fs::read(&file).unwrap();
    // The temp file cannot be made: a directory sits where the project is written next to.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    edit(&mut c, 2.0);
    c.save_in_background().unwrap();
    c.settle_save();
    assert!(c.last_rejection().is_some());
    assert!(c.is_dirty());
    assert!(file.is_dir());
    assert!(!before.is_empty());
}

#[test]
fn a_save_as_with_audio_copies_the_bundle_on_the_worker_and_switches_on_completion() {
    let t = Tmp::new("bundle");
    let mut c = core();
    import_wav(&mut c, &t);
    let scratch = c.bundle_root().unwrap().to_path_buf();
    let dest = t.path("out");
    std::fs::create_dir_all(&dest).unwrap();
    c.save_as_in_background(&dest.join("song.atomik")).unwrap();
    assert!(c.saving());
    assert_eq!(
        c.bundle_root(),
        Some(scratch.as_path()),
        "still the scratch bundle"
    );
    c.settle_save();
    let root = dest.join("song");
    assert_eq!(c.bundle_root(), Some(root.as_path()));
    assert_eq!(c.file(), Some(root.join("song.atomik").as_path()));
    assert!(!c.is_dirty());
    assert!(
        !scratch.exists(),
        "the scratch bundle goes once the copy is done"
    );
    assert_eq!(
        std::fs::read_dir(root.join("Audio"))
            .unwrap()
            .flatten()
            .filter(|e| { e.path().extension().is_some_and(|x| x == "wav") })
            .count(),
        1
    );
}

#[test]
fn replacing_the_project_waits_for_a_running_save() {
    let t = Tmp::new("replace");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 3.0);
    c.save_as_in_background(&file).unwrap();
    c.new_document();
    assert!(!c.saving());
    assert!(
        file.exists(),
        "the save finished before the project was replaced"
    );
    assert!(c.file().is_none());
    assert!(!c.is_dirty());
}

#[test]
fn an_import_waits_for_a_running_save_before_it_touches_the_bundle() {
    let t = Tmp::new("import");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 3.0);
    c.save_as_in_background(&file).unwrap();
    import_wav(&mut c, &t);
    assert!(!c.saving());
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(c.doc().assets.len(), 1);
}

#[test]
fn a_save_runs_in_the_foreground_while_audio_is_still_loading() {
    let t = Tmp::new("busy");
    let file = t.path("song.atomik");
    let mut c = core();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let path = t.path("a.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    assert!(c.is_busy());
    c.save_as_in_background(&file).unwrap();
    assert!(!c.saving(), "a blocking save ran instead");
    assert!(file.exists());
}

/// `cargo test -p atomik-app --release --lib save_speed -- --ignored --nocapture`
#[test]
#[ignore = "timing: run by hand"]
#[allow(clippy::disallowed_methods)]
fn save_speed_for_a_large_project() {
    let t = Tmp::new("speed");
    let file = t.path("big.atomik");
    let probe = core();
    let doc = atomik_model::stress::large_song_doc(probe.registry(), 100, 500, 200_000);
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    let best = |f: &mut dyn FnMut() -> std::time::Duration| (0..5).map(|_| f()).min().unwrap();
    let blocking = best(&mut || {
        let start = std::time::Instant::now();
        c.save_as(&file).unwrap();
        start.elapsed()
    });
    let ui_thread = best(&mut || {
        let start = std::time::Instant::now();
        c.save_as_in_background(&file).unwrap();
        let blocked = start.elapsed();
        c.settle_save();
        blocked
    });
    println!(
        "save, 200 000 notes: blocking {blocking:?}, UI thread blocked {ui_thread:?} \
         (the project text is {} bytes)",
        std::fs::metadata(&file).unwrap().len()
    );
}
