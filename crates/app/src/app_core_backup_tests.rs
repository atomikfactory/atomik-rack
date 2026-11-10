//! Backup copies in the app core: a save makes the copy and its audio on a worker, audio is kept
//! once, pruning keeps the newest copies and the audio they name, a failure never touches the
//! save and says so once, autosave snapshots make no backup, and a copy restores as a new unsaved
//! song. All fixtures are procedural and live in temporary folders that are removed afterwards.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{DeviceKey, empty_doc};
use atomik_ui::ImportTarget;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-backup-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
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

/// Imports one audio file made from `seed` and returns its hash.
fn import(c: &mut AppCore<RecordingSink>, t: &Tmp, seed: f32) -> String {
    let frames = 9_600;
    let data: Vec<f32> = (0..frames)
        .map(|i| ((i as f32) * 0.02 + seed).sin() * 0.4)
        .collect();
    let wav = t.path(&format!("a{seed}.wav"));
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let before: BTreeSet<String> = c.doc().assets.keys().cloned().collect();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    c.doc()
        .assets
        .keys()
        .find(|h| !before.contains(*h))
        .expect("a new asset")
        .clone()
}

fn edit(c: &mut AppCore<RecordingSink>, key: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".to_string(),
        key: Some(DeviceKey(key.to_string())),
    })
    .unwrap();
}

/// A song with `seeds.len()` audio files, saved as `t/home/song.atomik` with backups on in
/// `t/Backups`.
fn song(t: &Tmp, seeds: &[f32], keep: u32, audio: bool) -> (AppCore<RecordingSink>, Vec<String>) {
    let mut c = core();
    c.set_backup(Some(t.path("Backups")), keep, audio);
    let hashes = seeds.iter().map(|s| import(&mut c, t, *s)).collect();
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    c.wait_background();
    (c, hashes)
}

fn copies(t: &Tmp) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(t.path("Backups").join("song"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".atomik"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn backed_up_audio(t: &Tmp) -> BTreeSet<String> {
    std::fs::read_dir(t.path("Backups").join("song").join("Audio"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".wav"))
                .collect()
        })
        .unwrap_or_default()
}

fn wavs(hashes: &[String]) -> BTreeSet<String> {
    hashes.iter().map(|h| format!("{h}.wav")).collect()
}

// ------------------------------------------------------------------ the copy

#[test]
fn a_save_makes_the_copy_and_the_audio_on_a_worker() {
    let t = Tmp::new("save");
    let (mut c, hashes) = song(&t, &[1.0, 2.0], 10, true);
    // Save As made the first copy.
    assert_eq!(copies(&t).len(), 1);
    assert_eq!(backed_up_audio(&t), wavs(&hashes));
    let name = copies(&t).remove(0);
    assert!(name.starts_with("song 20"), "{name}");
    // The copy is the saved project, byte for byte.
    let saved = std::fs::read_to_string(c.file().unwrap()).unwrap();
    let copy = t.path("Backups").join("song").join(&name);
    assert_eq!(std::fs::read_to_string(copy).unwrap(), saved);
    // A second save makes a second copy.
    edit(&mut c, "extra");
    c.save().unwrap();
    assert!(c.backup_running() || copies(&t).len() == 2);
    c.wait_background();
    assert_eq!(copies(&t).len(), 2);
    assert!(!c.backup_running());
}

#[test]
fn the_name_of_a_copy_is_the_project_name_and_the_time() {
    let t = Tmp::new("name");
    let (_c, _) = song(&t, &[], 10, true);
    let name = copies(&t).remove(0);
    let stamp = name
        .strip_prefix("song ")
        .and_then(|n| n.strip_suffix(".atomik"))
        .unwrap();
    // `2026-11-10 093112`.
    let b = stamp.as_bytes();
    assert_eq!(b.len(), 17, "{stamp}");
    assert!(b[4] == b'-' && b[7] == b'-' && b[10] == b' ');
    assert!(
        b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7 | 10) || c.is_ascii_digit())
    );
}

#[test]
fn without_a_folder_no_backup_is_made_and_no_worker_runs() {
    let t = Tmp::new("off");
    let mut c = core();
    import(&mut c, &t, 1.0);
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    assert!(!c.backup_running());
    assert_eq!(c.backup_folder(), None);
    assert!(!t.path("Backups").exists());
}

#[test]
fn without_audio_only_the_project_is_copied() {
    let t = Tmp::new("no-audio");
    let (_c, _) = song(&t, &[1.0], 10, false);
    assert_eq!(copies(&t).len(), 1);
    assert!(backed_up_audio(&t).is_empty());
}

#[test]
fn audio_is_copied_once_however_many_saves_name_it() {
    let t = Tmp::new("dedup");
    let (mut c, hashes) = song(&t, &[1.0], 10, true);
    let file = t
        .path("Backups")
        .join("song")
        .join("Audio")
        .join(format!("{}.wav", hashes[0]));
    let stamp = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    for key in ["a", "b"] {
        edit(&mut c, key);
        c.save().unwrap();
        c.wait_background();
    }
    assert_eq!(copies(&t).len(), 3);
    assert_eq!(backed_up_audio(&t), wavs(&hashes));
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), stamp);
}

#[test]
fn pruning_keeps_the_newest_copies_and_the_audio_they_name() {
    let t = Tmp::new("prune");
    let (mut c, hashes) = song(&t, &[1.0], 2, true);
    // A second song audio arrives, and later the first one is removed from the song.
    let second = import(&mut c, &t, 2.0);
    c.save().unwrap();
    c.wait_background();
    edit(&mut c, "x");
    c.save().unwrap();
    c.wait_background();
    // Three saves, two kept.
    assert_eq!(copies(&t).len(), 2);
    // The newest two copies both name both files: all the audio stays.
    assert_eq!(backed_up_audio(&t), wavs(&[hashes[0].clone(), second]));
    // Every kept copy finds its audio.
    for name in copies(&t) {
        let text = std::fs::read_to_string(t.path("Backups").join("song").join(name)).unwrap();
        for hash in atomik_audio_file::scan_assets(&text) {
            assert!(backed_up_audio(&t).contains(&format!("{hash}.wav")));
        }
    }
}

#[test]
fn a_settings_change_applies_to_the_next_save() {
    let t = Tmp::new("settings");
    let mut c = core();
    assert_eq!(
        (c.backup_folder(), c.backup_keep(), c.backup_audio()),
        (None, 10, true)
    );
    c.set_backup(Some(t.path("Backups")), 500, false);
    assert_eq!(c.backup_folder(), Some(t.path("Backups").as_path()));
    assert_eq!((c.backup_keep(), c.backup_audio()), (50, false));
    c.set_backup(Some(PathBuf::new()), 0, true);
    assert_eq!((c.backup_folder(), c.backup_keep()), (None, 1));
}

// ------------------------------------------------------------------ failures

#[test]
fn a_failed_backup_never_touches_the_save_and_the_toast_shows_once() {
    let t = Tmp::new("fail");
    // A file where the backup folder should be: the folder cannot be made.
    std::fs::write(t.path("Backups"), "in the way").unwrap();
    let mut c = core();
    c.set_backup(Some(t.path("Backups")), 10, true);
    import(&mut c, &t, 1.0);
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    c.wait_background();
    // The save is whole.
    assert!(c.file().is_some_and(Path::exists));
    assert!(!c.is_dirty());
    let message = c.last_rejection().unwrap().to_owned();
    assert!(message.starts_with("Backup failed: "), "{message}");
    // The next failures stay in the log: the toast is not shown again.
    c.report("something else");
    edit(&mut c, "again");
    c.save().unwrap();
    c.wait_background();
    assert!(!c.is_dirty());
    assert_eq!(c.last_rejection(), Some("something else"));
}

#[test]
fn a_backup_folder_that_comes_back_works_again_without_a_second_toast() {
    let t = Tmp::new("back");
    std::fs::write(t.path("Backups"), "in the way").unwrap();
    let mut c = core();
    c.set_backup(Some(t.path("Backups")), 10, true);
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    c.wait_background();
    assert!(c.last_rejection().unwrap().starts_with("Backup failed: "));
    std::fs::remove_file(t.path("Backups")).unwrap();
    c.report("clear");
    edit(&mut c, "again");
    c.save().unwrap();
    c.wait_background();
    assert_eq!(copies(&t).len(), 1);
    assert_eq!(c.last_rejection(), Some("clear"));
}

// ------------------------------------------------------------------ autosave

#[test]
fn autosave_snapshots_make_no_backup() {
    let t = Tmp::new("auto");
    let recovery = t.path("Recovery");
    std::fs::create_dir_all(&recovery).unwrap();
    let mut c = core();
    c.configure_plugins(PluginConfig {
        paths: PluginPaths {
            recovery: Some(recovery.clone()),
            ..PluginPaths::default()
        },
        ..PluginConfig::default()
    });
    c.set_backup(Some(t.path("Backups")), 10, true);
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    c.wait_background();
    let before = copies(&t);
    assert_eq!(before.len(), 1);
    // A change, then the autosave period passes: a snapshot is written.
    edit(&mut c, "a");
    c.plugin_tick(10_000.0);
    c.plugin_tick(10_000.0 + 301.0);
    c.wait_recovery_writes();
    c.wait_background();
    let snapshots = std::fs::read_dir(&recovery)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".recover.atomik"))
        .count();
    assert_eq!(snapshots, 1);
    assert_eq!(copies(&t), before);
    assert!(!c.backup_running());
}

// ------------------------------------------------------------------ restore

#[test]
fn the_list_has_the_copies_newest_first_with_date_and_size() {
    let t = Tmp::new("list");
    let (mut c, _) = song(&t, &[1.0], 10, true);
    for key in ["a", "b"] {
        edit(&mut c, key);
        c.save().unwrap();
        c.wait_background();
    }
    c.start_backup_list().unwrap();
    assert!(c.backup_listing());
    let rows = loop {
        if let Some(rows) = c.take_backup_list() {
            break rows.unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(!c.backup_listing());
    assert_eq!(rows.len(), 3);
    // Newest first: the dates do not go up, and each has a size.
    let whens: Vec<&str> = rows.iter().map(|r| r.when.as_str()).collect();
    let mut sorted = whens.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(whens, sorted);
    for r in &rows {
        // `2026-11-10 09:31:12`.
        assert_eq!(r.when.len(), 19, "{}", r.when);
        assert_eq!(&r.when[13..14], ":");
        assert!(
            r.size.ends_with("KB") || r.size.ends_with("bytes"),
            "{}",
            r.size
        );
        assert!(r.path.is_file());
    }
}

#[test]
fn the_list_says_why_when_it_cannot_be_made() {
    let t = Tmp::new("list-why");
    let mut c = core();
    assert!(
        c.start_backup_list()
            .unwrap_err()
            .contains("No backup folder")
    );
    c.set_backup(Some(t.path("Backups")), 10, true);
    // The song has no project file yet.
    assert!(
        c.start_backup_list()
            .unwrap_err()
            .contains("Save the song first")
    );
    // A folder with no copies lists nothing.
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    c.wait_background();
    std::fs::remove_dir_all(t.path("Backups")).unwrap();
    c.start_backup_list().unwrap();
    let rows = loop {
        if let Some(rows) = c.take_backup_list() {
            break rows.unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert!(rows.is_empty());
}

#[test]
fn a_restored_copy_opens_as_a_new_unsaved_song_with_its_audio() {
    let t = Tmp::new("restore");
    let (mut c, hashes) = song(&t, &[1.0, 2.0], 10, true);
    let copy = t.path("Backups").join("song").join(copies(&t).remove(0));
    // The song changes after the copy was made; the copy is the earlier state.
    edit(&mut c, "later");
    c.save().unwrap();
    c.wait_background();
    let project = c.file().unwrap().to_path_buf();
    let saved = std::fs::read_to_string(&project).unwrap();
    c.restore_backup(&copy).unwrap();
    // A new song: no file of its own, so Save asks for a place, and the copy and the project are
    // not touched.
    assert_eq!(c.file(), None);
    assert!(c.doc().device(&DeviceKey("later".to_owned())).is_none());
    assert_eq!(c.doc().referenced_assets().len(), hashes.len());
    assert_eq!(std::fs::read_to_string(&project).unwrap(), saved);
    assert!(copy.is_file());
    assert!(matches!(c.save(), Err(AppError::NoPath)));
    // The audio came along into the session folder.
    c.wait_background();
    let root = c.bundle_root().unwrap().to_path_buf();
    for h in &hashes {
        assert!(root.join("Audio").join(format!("{h}.wav")).is_file());
    }
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Backup copy opened"))
    );
}

#[test]
fn only_a_listed_copy_of_this_project_restores() {
    let t = Tmp::new("restore-refuse");
    let (mut c, _) = song(&t, &[1.0], 10, true);
    let project = c.file().unwrap().to_path_buf();
    // The project file itself, a stray file in the folder and a file elsewhere.
    let stray = t.path("Backups").join("song").join("notes.atomik");
    std::fs::write(&stray, std::fs::read_to_string(&project).unwrap()).unwrap();
    for path in [project.clone(), stray, t.path("nothing.atomik")] {
        assert!(c.restore_backup(&path).is_err(), "{}", path.display());
        assert_eq!(c.file(), Some(project.as_path()));
    }
    // Without a folder nothing restores.
    c.set_backup(None, 10, true);
    let copy = t.path("Backups").join("song").join(copies(&t).remove(0));
    assert!(c.restore_backup(&copy).is_err());
}
