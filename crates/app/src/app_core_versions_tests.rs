//! The version history: what a save keeps and prunes, Save Version, protecting, Restore, and
//! what Move Project, Save As and archives do with the versions folder.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{
    AudioData, Bundle, VERSION_EXTENSION, VersionKind, VersionStore, write_wav_f32,
};
use atomik_model::{Command, empty_doc};
use atomik_ui::ImportTarget;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-versions-{tag}-{}-{n}",
            std::process::id()
        ));
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

fn tune(c: &mut AppCore<RecordingSink>, cents: f32) {
    c.apply_command(Command::SetMasterTune(cents)).unwrap();
}

/// A song saved as `<tmp>/song.atomik` (no audio, so no bundle folder) with tune 1 cent.
fn saved_song(t: &Tmp) -> AppCore<RecordingSink> {
    let mut c = core();
    tune(&mut c, 1.0);
    c.save_as(&t.path("song.atomik")).unwrap();
    c
}

fn store(c: &AppCore<RecordingSink>) -> VersionStore {
    VersionStore::for_project(c.file().unwrap()).unwrap()
}

fn tune_of(text: &str, c: &AppCore<RecordingSink>) -> f32 {
    atomik_model::load_project(text, c.registry())
        .unwrap()
        .tune_cents
}

#[test]
fn a_first_save_makes_no_version_and_makes_no_folder() {
    let t = Tmp::new("first");
    let c = saved_song(&t);
    assert!(store(&c).entries().is_empty());
    assert!(!t.path("song Versions").exists());
    assert_eq!(c.versions().unwrap(), Vec::new());
}

#[test]
fn every_save_keeps_the_file_it_replaces_as_an_automatic_version() {
    let t = Tmp::new("keep");
    let mut c = saved_song(&t);
    for cents in [2.0, 3.0, 4.0] {
        tune(&mut c, cents);
        c.save().unwrap();
    }
    let rows = c.versions().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.automatic && !r.protected));
    // Newest first: the file the last save replaced held 3 cents.
    let s = store(&c);
    assert_eq!(tune_of(&s.read(rows[0].id).unwrap(), &c), 3.0);
    assert_eq!(tune_of(&s.read(rows[2].id).unwrap(), &c), 1.0);
    assert!(rows[0].id > rows[1].id && rows[1].id > rows[2].id);
    // The versions are in a folder beside the project, not in the project file.
    assert!(t.path("song Versions").is_dir());
    let text = std::fs::read_to_string(t.path("song.atomik")).unwrap();
    assert_eq!(tune_of(&text, &c), 4.0);
}

#[test]
fn saving_what_is_already_saved_adds_no_version() {
    let t = Tmp::new("unchanged");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save().unwrap();
    assert_eq!(c.versions().unwrap().len(), 1);
    c.save().unwrap();
    c.save().unwrap();
    assert_eq!(
        c.versions().unwrap().len(),
        1,
        "an unchanged save keeps nothing new"
    );
}

#[test]
fn a_background_save_keeps_the_version_too() {
    let t = Tmp::new("background");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save_in_background().unwrap();
    c.settle_save();
    assert_eq!(c.versions().unwrap().len(), 1);
}

#[test]
fn the_newest_automatic_versions_are_kept_and_the_older_ones_pruned() {
    let t = Tmp::new("prune");
    let mut c = saved_song(&t);
    c.set_keep_versions(3);
    for i in 0..8 {
        tune(&mut c, 10.0 + i as f32);
        c.save().unwrap();
    }
    let rows = c.versions().unwrap();
    assert_eq!(rows.len(), 3);
    let s = store(&c);
    let cents: Vec<f32> = rows
        .iter()
        .map(|r| tune_of(&s.read(r.id).unwrap(), &c))
        .collect();
    assert_eq!(cents, vec![16.0, 15.0, 14.0], "the newest three");
    let files: Vec<_> = std::fs::read_dir(t.path("song Versions"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(VERSION_EXTENSION))
        .collect();
    assert_eq!(files.len(), 3, "{files:?}");
}

#[test]
fn named_and_protected_versions_and_other_files_survive_pruning() {
    let t = Tmp::new("survive");
    let mut c = saved_song(&t);
    c.set_keep_versions(2);
    tune(&mut c, 2.0);
    c.save().unwrap();
    let first = c.versions().unwrap()[0].id;
    c.set_version_protected(first, true).unwrap();
    tune(&mut c, 3.0);
    let named = c.save_version("Before the mix").unwrap();
    // Strangers in the versions folder, and a file next to the project.
    let folder = t.path("song Versions");
    std::fs::write(folder.join("notes.txt"), "mine").unwrap();
    std::fs::write(
        folder.join(format!("song v0777.{VERSION_EXTENSION}")),
        "stray",
    )
    .unwrap();
    std::fs::write(t.path("other.atomik"), "another project").unwrap();
    for i in 0..10 {
        tune(&mut c, 20.0 + i as f32);
        c.save().unwrap();
    }
    let rows = c.versions().unwrap();
    assert!(rows.iter().any(|r| r.id == first && r.protected));
    let named_row = rows.iter().find(|r| r.id == named).unwrap();
    assert!(!named_row.automatic && named_row.protected);
    assert_eq!(named_row.name, "Before the mix");
    let automatic = rows.iter().filter(|r| r.automatic && !r.protected).count();
    assert_eq!(automatic, 2);
    let s = store(&c);
    assert_eq!(tune_of(&s.read(first).unwrap(), &c), 1.0);
    assert_eq!(tune_of(&s.read(named).unwrap(), &c), 3.0);
    assert_eq!(
        std::fs::read_to_string(folder.join("notes.txt")).unwrap(),
        "mine"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join(format!("song v0777.{VERSION_EXTENSION}"))).unwrap(),
        "stray"
    );
    assert_eq!(
        std::fs::read_to_string(t.path("other.atomik")).unwrap(),
        "another project"
    );
}

#[test]
fn a_protected_version_can_be_unprotected_and_then_ages_out() {
    let t = Tmp::new("unprotect");
    let mut c = saved_song(&t);
    c.set_keep_versions(1);
    tune(&mut c, 2.0);
    c.save().unwrap();
    let id = c.versions().unwrap()[0].id;
    c.set_version_protected(id, true).unwrap();
    for i in 0..3 {
        tune(&mut c, 5.0 + i as f32);
        c.save().unwrap();
    }
    assert!(c.versions().unwrap().iter().any(|r| r.id == id));
    c.set_version_protected(id, false).unwrap();
    tune(&mut c, 50.0);
    c.save().unwrap();
    assert!(!c.versions().unwrap().iter().any(|r| r.id == id));
}

#[test]
fn save_version_keeps_the_song_as_it_is_without_saving_it() {
    let t = Tmp::new("named");
    let mut c = saved_song(&t);
    let on_disk = std::fs::read(t.path("song.atomik")).unwrap();
    tune(&mut c, 9.0);
    let id = c.save_version("  Idea A\tnow ").unwrap();
    assert!(c.is_dirty(), "Save Version does not save the song");
    assert_eq!(std::fs::read(t.path("song.atomik")).unwrap(), on_disk);
    let rows = c.versions().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Idea A now");
    assert!(!rows[0].automatic);
    assert_eq!(tune_of(&store(&c).read(id).unwrap(), &c), 9.0);
}

#[test]
fn versions_need_a_project_file() {
    let mut c = core();
    assert!(matches!(c.versions(), Err(AppError::NoPath)));
    assert!(matches!(c.save_version("x"), Err(AppError::NoPath)));
    assert!(matches!(c.restore_version(1), Err(AppError::NoPath)));
    assert!(matches!(
        c.set_version_protected(1, true),
        Err(AppError::NoPath)
    ));
}

#[test]
fn restoring_opens_the_version_unsaved_and_keeps_the_current_state_first() {
    let t = Tmp::new("restore");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save().unwrap();
    tune(&mut c, 3.0);
    c.save().unwrap();
    // The version of the 1-cent file, then an unsaved change on top of the saved 3 cents.
    let old = c.versions().unwrap().last().unwrap().id;
    tune(&mut c, 77.0);
    c.restore_version(old).unwrap();
    assert_eq!(c.doc().tune_cents, 1.0);
    assert_eq!(
        c.file(),
        Some(t.path("song.atomik").as_path()),
        "the song keeps its file"
    );
    assert!(c.is_dirty(), "the restored song is not saved yet");
    assert!(!c.undo(), "the restored song starts a fresh history");
    // The state before the restore, unsaved change included, was kept.
    let rows = c.versions().unwrap();
    let before = &rows[0];
    assert!(before.automatic);
    assert_eq!(before.name, "Before restoring a version");
    assert_eq!(tune_of(&store(&c).read(before.id).unwrap(), &c), 77.0);
    // Restoring that one undoes the restore.
    let before = before.id;
    c.restore_version(before).unwrap();
    assert_eq!(c.doc().tune_cents, 77.0);
}

#[test]
fn saving_a_restored_song_loses_nothing_and_adds_no_duplicate() {
    let t = Tmp::new("restore-save");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save().unwrap();
    let old = c.versions().unwrap()[0].id;
    // The saved file holds 2 cents; restoring keeps that state as a version first.
    c.restore_version(old).unwrap();
    let before = c.versions().unwrap().len();
    c.save().unwrap();
    assert!(!c.is_dirty());
    assert_eq!(c.doc().tune_cents, 1.0);
    // The file the save replaced is the state that restore already kept: no copy of it.
    assert_eq!(c.versions().unwrap().len(), before);
    let s = store(&c);
    let kept: Vec<f32> = c
        .versions()
        .unwrap()
        .iter()
        .map(|r| tune_of(&s.read(r.id).unwrap(), &c))
        .collect();
    assert!(kept.contains(&2.0) && kept.contains(&1.0), "{kept:?}");
}

#[test]
fn restoring_a_version_that_is_missing_or_damaged_changes_nothing() {
    let t = Tmp::new("restore-bad");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save().unwrap();
    tune(&mut c, 5.0);
    let before_rows = c.versions().unwrap();
    assert!(c.restore_version(999).is_err());
    let id = before_rows[0].id;
    let path = store(&c).path_of(id).unwrap();
    std::fs::write(&path, "this is not a project").unwrap();
    assert!(c.restore_version(id).is_err());
    assert_eq!(c.doc().tune_cents, 5.0);
    assert!(c.is_dirty());
    assert_eq!(
        c.versions().unwrap(),
        before_rows,
        "no version was kept either"
    );
}

#[test]
fn the_audio_a_kept_version_uses_survives_the_next_save() {
    let t = Tmp::new("audio");
    let mut c = core();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let wav = t.path("a.wav");
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    std::fs::create_dir_all(t.path("proj")).unwrap();
    c.save_as(&t.path("proj").join("proj.atomik")).unwrap();
    let root = t.path("proj");
    let mut bundle = Bundle::open(&root).unwrap();
    let mut stray = |seed: f32| {
        bundle
            .add_recording(&AudioData::new(48_000, vec![vec![seed; 700]]).unwrap(), "x")
            .unwrap()
            .hash
    };
    let (held, loose) = (stray(0.25), stray(0.5));
    // A kept version uses `held`; nothing uses `loose`.
    let mut versions = store(&c);
    versions
        .add(
            VersionKind::Named,
            "uses held",
            "text",
            &BTreeSet::from([held.clone()]),
            1,
        )
        .unwrap();
    tune(&mut c, 3.0);
    c.save().unwrap();
    c.wait_background();
    let wav_of = |h: &str| root.join("Audio").join(format!("{h}.wav"));
    assert!(wav_of(&held).exists(), "a version still needs it");
    assert!(!wav_of(&loose).exists(), "unused audio is tidied as before");
    assert!(
        root.join("Versions").is_dir(),
        "a bundle keeps its versions inside it"
    );
}

#[test]
fn audio_of_a_removed_clip_stays_while_a_version_uses_it_and_goes_when_none_does() {
    use atomik_model::SongCommand;
    let t = Tmp::new("removed-clip");
    let mut c = core();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let wav = t.path("a.wav");
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    let hash = c.doc().assets.keys().next().cloned().unwrap();
    c.save_as(&t.path("song.atomik")).unwrap();
    let file = t.path("song").join("song.atomik");
    let asset = t.path("song").join("Audio").join(format!("{hash}.wav"));
    let mut fresh = core();
    fresh.open(&file).unwrap();
    fresh.wait_background();
    let id = fresh.doc().song.tracks[0].lanes[0].clips[0].id;
    fresh
        .apply_command(Command::Song(SongCommand::RemoveClip { id }))
        .unwrap();
    fresh.save().unwrap();
    // The file this save replaced uses the audio and is kept as a version.
    let mut next = core();
    next.open(&file).unwrap();
    next.wait_background();
    assert!(next.doc().assets.is_empty());
    tune(&mut next, 4.0);
    next.save().unwrap();
    assert!(asset.exists(), "the first version still uses it");
    // Restoring that version brings the clip back and it has its audio.
    let first = next.versions().unwrap().last().unwrap().id;
    next.restore_version(first).unwrap();
    next.wait_background();
    assert!(next.doc().assets.contains_key(&hash));
    assert!(next.song_assets().contains_key(&hash));
    // Once pruning has dropped every version that used it, the next save deletes the file.
    let mut last = core();
    last.set_keep_versions(1);
    last.open(&file).unwrap();
    last.wait_background();
    for i in 0..3 {
        tune(&mut last, 10.0 + i as f32);
        last.save().unwrap();
    }
    assert!(!asset.exists());
}

#[test]
fn a_version_file_is_not_another_project_in_a_shared_folder() {
    let t = Tmp::new("shared");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save().unwrap();
    let version = store(&c).path_of(c.versions().unwrap()[0].id).unwrap();
    assert!(!super::super::is_project_path(&version));
}

#[test]
fn save_as_to_a_new_place_starts_a_history_of_its_own() {
    let t = Tmp::new("save-as");
    let mut c = saved_song(&t);
    tune(&mut c, 2.0);
    c.save().unwrap();
    assert_eq!(c.versions().unwrap().len(), 1);
    c.save_as(&t.path("copy.atomik")).unwrap();
    assert!(c.versions().unwrap().is_empty());
    assert_eq!(
        VersionStore::for_project(&t.path("song.atomik"))
            .unwrap()
            .entries()
            .len(),
        1,
        "the first project keeps its own"
    );
}

#[test]
fn move_project_carries_the_versions_along() {
    let t = Tmp::new("move");
    let mut c = core();
    c.set_recent_path(t.path("recent.json"));
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let wav = t.path("a.wav");
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    std::fs::create_dir_all(t.path("old")).unwrap();
    c.save_as(&t.path("old").join("song.atomik")).unwrap();
    tune(&mut c, 2.0);
    c.save().unwrap();
    tune(&mut c, 3.0);
    let named = c.save_version("Keeper").unwrap();
    let first = c
        .versions()
        .unwrap()
        .iter()
        .find(|r| r.automatic)
        .unwrap()
        .id;
    c.set_version_protected(first, true).unwrap();
    let old_root = c.bundle_root().unwrap().to_path_buf();
    assert!(old_root.join("Versions").is_dir());

    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();

    let new_root = t.path("new").join("song");
    assert_eq!(c.file(), Some(new_root.join("song.atomik").as_path()));
    assert!(
        !old_root.exists(),
        "the old folder is gone, versions included"
    );
    let rows = c.versions().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(new_root.join("Versions").is_dir());
    let keeper = rows.iter().find(|r| r.name == "Keeper").unwrap();
    assert!(!keeper.automatic);
    let moved = rows.iter().find(|r| r.automatic).unwrap();
    assert!(moved.protected);
    let s = store(&c);
    assert_eq!(tune_of(&s.read(keeper.id).unwrap(), &c), 3.0);
    let _ = named;
}

#[test]
fn an_archive_leaves_the_versions_out() {
    let t = Tmp::new("archive");
    let mut c = core();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let wav = t.path("a.wav");
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    std::fs::create_dir_all(t.path("home")).unwrap();
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    tune(&mut c, 2.0);
    c.save().unwrap();
    c.save_version("Named").unwrap();
    assert!(!c.versions().unwrap().is_empty());
    c.start_archive_export(t.path("song.atomikz")).unwrap();
    c.wait_background();

    let out = t.path("unpacked");
    let extracted = atomik_audio_file::extract_archive(
        &t.path("song.atomikz"),
        &out,
        &atomik_audio_file::ArchiveLimits::default(),
        &std::sync::atomic::AtomicBool::new(false),
        &AtomicU64::new(0),
    )
    .unwrap();
    assert!(!extracted.root.join("Versions").exists());
    let mut all = Vec::new();
    collect(&extracted.root, &mut all);
    assert!(
        all.iter()
            .all(|p| !p.to_string_lossy().contains(VERSION_EXTENSION)),
        "{all:?}"
    );
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        }
        out.push(path);
    }
}

#[test]
fn collect_files_copies_audio_only_and_leaves_the_versions_where_they_are() {
    let t = Tmp::new("collect");
    let mut c = core();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let wav = t.path("a.wav");
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![wav], ImportTarget::NewTracks, 0);
    c.wait_background();
    std::fs::create_dir_all(t.path("home")).unwrap();
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    tune(&mut c, 2.0);
    c.save().unwrap();
    c.save_version("Named").unwrap();
    let before = c.versions().unwrap();
    c.collect_files().unwrap();
    c.wait_background();
    let root = c.bundle_root().unwrap().to_path_buf();
    let after = c.versions().unwrap();
    assert!(before.iter().all(|b| after.iter().any(|a| a.id == b.id)));
    assert!(root.join("Versions").is_dir());
    let audio: Vec<_> = std::fs::read_dir(root.join("Audio"))
        .unwrap()
        .flatten()
        .collect();
    assert!(
        audio
            .iter()
            .all(|e| !e.file_name().to_string_lossy().contains("atomikver"))
    );
}
