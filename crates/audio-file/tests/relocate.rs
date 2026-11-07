//! Moving a project: the plan of a collection, cancelling it, verifying a copy and retiring the
//! old project. Nothing may be removed before the copy is proved, and nothing another project
//! uses may be removed at all.
#![allow(missing_docs)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use atomik_audio_file::{
    AudioData, AudioFileError, Bundle, CollectProgress, RetireReport, VersionKind, VersionStore,
    retire_old_project, verify_copy,
};
use common::*;

fn recording(frames: usize, seed: f32) -> AudioData {
    let l: Vec<f32> = (0..frames)
        .map(|i| ((i as f32) * 0.01 + seed).sin() * 0.5)
        .collect();
    AudioData::new(48_000, vec![l]).unwrap()
}

/// A project folder `<dir>/<name>` with `<name>.atomik` and the assets `seeds` in `Audio/`.
struct Old {
    root: PathBuf,
    file: PathBuf,
    hashes: Vec<String>,
}

fn old_project(dir: &Path, name: &str, seeds: &[f32]) -> Old {
    let root = dir.join(name);
    let mut bundle = Bundle::create(&root).unwrap();
    let hashes = seeds
        .iter()
        .map(|s| {
            bundle
                .add_recording(&recording(5000, *s), "x")
                .unwrap()
                .hash
        })
        .collect();
    let file = root.join(format!("{name}.atomik"));
    std::fs::write(&file, "old project text").unwrap();
    Old { root, file, hashes }
}

/// Copies `old` to `<dir>/<name>` the way Save As does and returns the new file.
fn copy_to(old: &Old, dir: &Path, name: &str, text: &str) -> (PathBuf, PathBuf) {
    let root = dir.join(name);
    let new = Bundle::create(&root).unwrap();
    let wanted: BTreeSet<String> = old.hashes.iter().cloned().collect();
    new.collect_from(std::slice::from_ref(&old.root), &wanted)
        .unwrap();
    let file = root.join(format!("{name}.atomik"));
    std::fs::write(&file, text).unwrap();
    (file, root)
}

fn wanted(old: &Old) -> BTreeSet<String> {
    old.hashes.iter().cloned().collect()
}

#[test]
fn the_plan_says_what_a_collection_would_copy_and_writes_nothing() {
    let dir = tmp_dir("reloc-plan");
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    let own = Bundle::create(&dir.join("Own")).unwrap();
    let ghost = "e".repeat(64);
    let mut want = wanted(&old);
    want.insert(ghost.clone());
    let plan = own.plan_collect(std::slice::from_ref(&old.root), &want);
    assert_eq!(plan.to_copy.len(), 2);
    assert!(plan.bytes() > 0);
    assert_eq!(plan.missing, vec![ghost]);
    assert!(plan.present.is_empty());
    for hash in &old.hashes {
        assert!(!own.exists(hash), "a plan copies nothing");
    }
    // It agrees with what the collection then does.
    let report = own
        .collect_from(std::slice::from_ref(&old.root), &want)
        .unwrap();
    assert_eq!(report.bytes, plan.bytes());
    assert_eq!(report.copied.len(), plan.to_copy.len());
    let again = own.plan_collect(std::slice::from_ref(&old.root), &want);
    assert!(again.to_copy.is_empty());
    assert_eq!(again.present.len(), 2);
}

#[test]
fn a_cancelled_collection_stops_cleanly_and_a_second_run_finishes_it() {
    let dir = tmp_dir("reloc-cancel");
    let old = old_project(&dir, "Old", &[0.1, 0.2, 0.3]);
    let own = Bundle::create(&dir.join("Own")).unwrap();
    let progress = CollectProgress::new();
    progress.cancel();
    let report = own
        .collect_from_with(std::slice::from_ref(&old.root), &wanted(&old), &progress)
        .unwrap();
    assert!(report.cancelled);
    assert!(report.copied.is_empty());
    let leftovers: Vec<_> = std::fs::read_dir(own.audio_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .collect();
    assert!(leftovers.is_empty(), "no file, and no half-written one");

    let fresh = CollectProgress::new();
    let report = own
        .collect_from_with(std::slice::from_ref(&old.root), &wanted(&old), &fresh)
        .unwrap();
    assert!(!report.cancelled);
    assert_eq!(report.copied.len(), 3);
    assert_eq!(fresh.files(), (3, 3));
}

#[test]
fn a_collection_cancelled_halfway_keeps_what_was_copied_whole() {
    let dir = tmp_dir("reloc-halfway");
    let old = old_project(&dir, "Old", &[0.1, 0.2, 0.3, 0.4]);
    let own = Bundle::create(&dir.join("Own")).unwrap();
    // A source that cancels the run when its first file is read cannot be built from outside, so
    // cancel from a thread as soon as anything is copied.
    let progress = std::sync::Arc::new(CollectProgress::new());
    let watcher = {
        let progress = progress.clone();
        let audio = own.audio_dir();
        std::thread::spawn(move || {
            for _ in 0..20_000 {
                let wavs = std::fs::read_dir(&audio)
                    .map(|d| {
                        d.flatten()
                            .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
                            .count()
                    })
                    .unwrap_or(0);
                if wavs > 0 {
                    break;
                }
                std::thread::yield_now();
            }
            progress.cancel();
        })
    };
    let report = own
        .collect_from_with(std::slice::from_ref(&old.root), &wanted(&old), &progress)
        .unwrap();
    watcher.join().unwrap();
    // Whatever the timing, every file under a real name is a whole copy of its original.
    for hash in &report.copied {
        assert_eq!(
            std::fs::read(own.asset_path(hash)).unwrap(),
            std::fs::read(Bundle::open(&old.root).unwrap().asset_path(hash)).unwrap()
        );
    }
    for entry in std::fs::read_dir(own.audio_dir()).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.ends_with(".tmp~"),
            "no temporary file is left: {name}"
        );
    }
    assert_eq!(report.cancelled, report.copied.len() < 4);
}

#[test]
fn a_verified_move_removes_the_old_project_folder() {
    let dir = tmp_dir("reloc-ok");
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap();
    assert!(report.refused.is_none() && !report.shared);
    assert!(report.folder_removed);
    assert_eq!(report.removed_files, 3, "the file and two audio files");
    assert!(!old.root.exists());
    for hash in &old.hashes {
        assert!(Bundle::open(&new_root).unwrap().exists(hash));
    }
    assert!(new_file.is_file());
}

#[test]
fn a_copy_that_differs_or_is_missing_removes_nothing() {
    let dir = tmp_dir("reloc-bad");
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    let before = snapshot(&old.root);
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let new = Bundle::open(&new_root).unwrap();

    // One byte of a copy differs (same length).
    let victim = new.asset_path(&old.hashes[0]);
    let mut bytes = std::fs::read(&victim).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x55;
    std::fs::write(&victim, &bytes).unwrap();
    let err = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap_err();
    assert!(matches!(err, AudioFileError::Unverified(_)), "{err}");
    assert_eq!(snapshot(&old.root), before, "the old project is untouched");
    assert!(verify_copy(&old.root, &new, &wanted(&old)).is_err());

    // A copy is truncated.
    std::fs::write(&victim, &bytes[..bytes.len() / 2]).unwrap();
    assert!(verify_copy(&old.root, &new, &wanted(&old)).is_err());

    // A copy is gone.
    std::fs::remove_file(&victim).unwrap();
    let err = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap_err();
    assert!(matches!(err, AudioFileError::Unverified(_)));
    assert_eq!(snapshot(&old.root), before);
}

#[test]
fn old_audio_with_no_new_folder_to_check_removes_nothing() {
    let dir = tmp_dir("reloc-nofolder");
    let old = old_project(&dir, "Old", &[0.3]);
    let before = snapshot(&old.root);
    let new_file = dir.join("Elsewhere").join("New.atomik");
    std::fs::create_dir_all(new_file.parent().unwrap()).unwrap();
    std::fs::write(&new_file, "new text").unwrap();
    let err = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        None,
        &wanted(&old),
    )
    .unwrap_err();
    assert!(matches!(err, AudioFileError::Unverified(_)), "{err}");
    assert_eq!(snapshot(&old.root), before, "the old project is untouched");
}

#[test]
fn a_new_project_file_that_is_not_what_was_written_removes_nothing() {
    let dir = tmp_dir("reloc-text");
    let old = old_project(&dir, "Old", &[0.1]);
    let before = snapshot(&old.root);
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "cut off");
    let err = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "the whole text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap_err();
    assert!(matches!(err, AudioFileError::Unverified(_)));
    assert_eq!(snapshot(&old.root), before);

    // No new file at all.
    std::fs::remove_file(&new_file).unwrap();
    assert!(
        retire_old_project(
            &old.file,
            Some(&old.root),
            &new_file,
            "the whole text",
            Some(&new_root),
            &wanted(&old),
        )
        .is_err()
    );
    assert_eq!(snapshot(&old.root), before);
}

#[test]
fn another_project_in_the_old_folder_keeps_its_audio() {
    let dir = tmp_dir("reloc-shared");
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    // A second project of the same folder, with audio that only it uses.
    let mut bundle = Bundle::open(&old.root).unwrap();
    let theirs = bundle
        .add_recording(&recording(6000, 0.9), "theirs")
        .unwrap()
        .hash;
    let other = old.root.join("Other.atomik");
    // It uses both of the audio files of the old project, and its own.
    let used = project_text(&[&old.hashes[0], &old.hashes[1], &theirs]);
    std::fs::write(&other, used).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");

    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap();
    assert!(report.shared && !report.folder_removed);
    assert_eq!(report.removed_files, 1, "only the old project file");
    assert_eq!(report.kept_audio, 2);
    assert!(!old.file.exists());
    assert!(other.is_file());
    let b = Bundle::open(&old.root).unwrap();
    assert!(b.exists(&theirs));
    for hash in &old.hashes {
        assert!(b.exists(hash), "shared audio stays where it is");
    }
}

/// A project text that names the audio `hashes`.
fn project_text(hashes: &[&str]) -> String {
    let list: Vec<String> = hashes.iter().map(|h| format!("\"{h}\"")).collect();
    format!("{{\"assets\": [{}]}}", list.join(", "))
}

/// Retires `old` with the copy `new_file` and the audio `old` holds as the audio in use.
fn retire(old: &Old, new_file: &Path, new_root: &Path) -> Result<RetireReport, AudioFileError> {
    retire_old_project(
        &old.file,
        Some(&old.root),
        new_file,
        "new text",
        Some(new_root),
        &wanted(old),
    )
}

#[test]
fn audio_that_no_other_project_uses_is_removed_and_the_rest_stays() {
    let dir = tmp_dir("reloc-shared-split");
    let old = old_project(&dir, "Old", &[0.1, 0.2, 0.3]);
    let other = old.root.join("Other.atomik");
    // Only the second file is shared.
    std::fs::write(&other, project_text(&[&old.hashes[1]])).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire(&old, &new_file, &new_root).unwrap();
    assert!(report.shared && report.unreadable.is_none());
    assert_eq!(report.kept_audio, 1);
    assert_eq!(
        report.removed_files, 3,
        "the project file and two audio files"
    );
    let b = Bundle::open(&old.root).unwrap();
    assert!(!b.exists(&old.hashes[0]) && !b.exists(&old.hashes[2]));
    assert!(!b.peaks_path(&old.hashes[0]).exists(), "its cache goes too");
    assert!(b.exists(&old.hashes[1]) && other.is_file());
    // The new place holds all of it.
    let n = Bundle::open(&new_root).unwrap();
    assert!(old.hashes.iter().all(|h| n.exists(h)));
}

#[test]
fn audio_a_kept_version_of_another_project_uses_stays() {
    let dir = tmp_dir("reloc-shared-version");
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    let other = old.root.join("Other.atomik");
    std::fs::write(&other, project_text(&[])).unwrap();
    // An older version of the other project used the first file.
    let mut store = VersionStore::for_project(&other).unwrap();
    let assets: BTreeSet<String> = [old.hashes[0].clone()].into();
    store
        .add(
            VersionKind::Named,
            "older",
            &project_text(&[&old.hashes[0]]),
            &assets,
            1,
        )
        .unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire(&old, &new_file, &new_root).unwrap();
    assert_eq!(report.kept_audio, 1);
    let b = Bundle::open(&old.root).unwrap();
    assert!(
        b.exists(&old.hashes[0]),
        "a version of the other project uses it"
    );
    assert!(!b.exists(&old.hashes[1]));
}

#[test]
fn a_project_that_cannot_be_read_keeps_all_of_the_old_audio() {
    let dir = tmp_dir("reloc-shared-unreadable");
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    // Not text: reading it fails.
    let other = old.root.join("Sub").join("Broken.atomik");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire(&old, &new_file, &new_root).unwrap();
    let why = report.unreadable.expect("it says what it could not read");
    assert!(why.contains("Broken.atomik"), "{why}");
    assert_eq!(report.kept_audio, 2);
    assert_eq!(report.removed_files, 1, "only the old project file");
    let b = Bundle::open(&old.root).unwrap();
    assert!(old.hashes.iter().all(|h| b.exists(h)));
}

#[test]
fn a_folder_too_deep_to_be_looked_into_keeps_all_of_the_old_audio() {
    let dir = tmp_dir("reloc-shared-deep");
    let old = old_project(&dir, "Old", &[0.1]);
    let mut deep = old.root.clone();
    for i in 0..14 {
        deep = deep.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire(&old, &new_file, &new_root).unwrap();
    assert!(report.unreadable.is_some());
    assert!(Bundle::open(&old.root).unwrap().exists(&old.hashes[0]));
}

#[test]
fn a_failed_check_removes_no_audio_even_when_others_share_the_folder() {
    let dir = tmp_dir("reloc-shared-unverified");
    let old = old_project(&dir, "Old", &[0.1]);
    std::fs::write(old.root.join("Other.atomik"), project_text(&[])).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    // The copy differs from the original.
    std::fs::write(
        Bundle::open(&new_root).unwrap().asset_path(&old.hashes[0]),
        b"damaged",
    )
    .unwrap();
    let before = snapshot(&old.root);
    let err = retire(&old, &new_file, &new_root);
    assert!(matches!(err, Err(AudioFileError::Unverified(_))));
    assert_eq!(snapshot(&old.root), before);
}

#[test]
fn another_project_in_a_subfolder_also_counts() {
    let dir = tmp_dir("reloc-nested-other");
    let old = old_project(&dir, "Old", &[0.1]);
    std::fs::create_dir_all(old.root.join("Sub").join("Deeper")).unwrap();
    let other = old.root.join("Sub").join("Deeper").join("Other.atomik");
    std::fs::write(&other, project_text(&[&old.hashes[0]])).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap();
    assert!(report.shared);
    assert!(other.is_file());
    assert!(Bundle::open(&old.root).unwrap().exists(&old.hashes[0]));
}

#[test]
fn files_the_project_does_not_own_keep_their_folder() {
    let dir = tmp_dir("reloc-keep");
    let old = old_project(&dir, "Old", &[0.1]);
    let note = old.root.join("notes.txt");
    std::fs::write(&note, "mine").unwrap();
    let stray = old.root.join("Audio").join("my-own-sample.wav");
    std::fs::write(&stray, "not an asset").unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap();
    assert!(!report.folder_removed);
    assert!(note.is_file() && stray.is_file());
    assert!(!old.file.exists());
    assert!(!Bundle::open(&old.root).unwrap().exists(&old.hashes[0]));
}

#[test]
fn the_same_place_or_a_place_inside_it_is_refused_and_nothing_is_removed() {
    let dir = tmp_dir("reloc-same");
    let old = old_project(&dir, "Old", &[0.1]);
    let before = snapshot(&old.root);
    let want = wanted(&old);
    // The same file.
    let same = retire_old_project(
        &old.file,
        Some(&old.root),
        &old.file,
        "old project text",
        Some(&old.root),
        &want,
    )
    .unwrap();
    assert!(same.refused.is_some());
    // A new project inside the old folder.
    let inner = old.root.join("Inner");
    let new = Bundle::create(&inner).unwrap();
    new.collect_from(std::slice::from_ref(&old.root), &want)
        .unwrap();
    let inner_file = inner.join("Inner.atomik");
    std::fs::write(&inner_file, "inner").unwrap();
    let before_inner = snapshot(&old.root);
    let nested = retire_old_project(
        &old.file,
        Some(&old.root),
        &inner_file,
        "inner",
        Some(&inner),
        &want,
    )
    .unwrap();
    assert!(nested.refused.is_some());
    assert_eq!(snapshot(&old.root), before_inner);
    assert_ne!(before, before_inner, "the inner copy was added by the test");
    // A new place around the old one.
    let around = retire_old_project(
        &old.file,
        Some(&old.root),
        &dir.join("Around.atomik"),
        "x",
        Some(&dir),
        &want,
    )
    .unwrap();
    assert!(around.refused.is_some());
    assert!(old.file.is_file());
}

#[test]
fn a_project_without_audio_moves_by_removing_only_its_file() {
    let dir = tmp_dir("reloc-flat");
    let old_file = dir.join("Flat.atomik");
    std::fs::write(&old_file, "old").unwrap();
    let keep = dir.join("keep.txt");
    std::fs::write(&keep, "mine").unwrap();
    let new_file = dir.join("Elsewhere").join("Flat.atomik");
    std::fs::create_dir_all(new_file.parent().unwrap()).unwrap();
    std::fs::write(&new_file, "new").unwrap();
    let report =
        retire_old_project(&old_file, None, &new_file, "new", None, &BTreeSet::new()).unwrap();
    assert_eq!(report.removed_files, 1);
    assert!(!old_file.exists() && keep.is_file() && new_file.is_file());
}

#[test]
fn audio_the_old_bundle_lacked_does_not_block_the_move() {
    let dir = tmp_dir("reloc-lost");
    let old = old_project(&dir, "Old", &[0.1]);
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let mut want = wanted(&old);
    want.insert("d".repeat(64));
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &want,
    )
    .unwrap();
    assert!(report.folder_removed);
}

/// Every file under `root` with its bytes.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push((p.clone(), std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

/// Removes the test folder when the test ends.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Makes `count` automatic versions and one named version of the project `file`.
#[test]
fn audio_a_version_file_left_out_of_its_index_uses_stays() {
    let dir = tmp_dir("reloc-shared-stray-version");
    let _cleanup = Cleanup(dir.clone());
    let old = old_project(&dir, "Old", &[0.1, 0.2]);
    // The old project's own versions name all of its audio: they go with it and keep nothing.
    let own = old.hashes.iter().cloned().collect::<BTreeSet<String>>();
    VersionStore::for_project(&old.file)
        .unwrap()
        .add(
            VersionKind::Named,
            "mine",
            &project_text(&[&old.hashes[0], &old.hashes[1]]),
            &own,
            1,
        )
        .unwrap();
    // Another project of the folder, with a version file its index does not list.
    let other = old.root.join("Other.atomik");
    std::fs::write(&other, project_text(&[])).unwrap();
    let stray = old.root.join("Versions").join("Other v0007.atomikver");
    std::fs::write(&stray, project_text(&[&old.hashes[1]])).unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "New", "new text");
    let report = retire(&old, &new_file, &new_root).unwrap();
    assert_eq!(report.kept_audio, 1);
    let b = Bundle::open(&old.root).unwrap();
    assert!(!b.exists(&old.hashes[0]), "nothing else uses it");
    assert!(
        b.exists(&old.hashes[1]),
        "the unlisted version still names it"
    );
    assert!(stray.is_file());
}

fn keep_versions(file: &Path, count: usize) -> VersionStore {
    let mut store = VersionStore::for_project(file).unwrap();
    let none = BTreeSet::new();
    for i in 0..count {
        store
            .add(
                VersionKind::Automatic,
                "",
                &format!("text {i}"),
                &none,
                i as u64,
            )
            .unwrap();
    }
    store
        .add(VersionKind::Named, "Named", "named text", &none, 99)
        .unwrap();
    store
}

#[test]
fn a_move_carries_the_versions_and_the_old_folder_still_goes() {
    let dir = tmp_dir("reloc-versions");
    let _cleanup = Cleanup(dir.clone());
    let old = old_project(&dir, "Old", &[0.1]);
    keep_versions(&old.file, 3);
    assert!(old.root.join("Versions").is_dir());
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "Old", "new text");
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap();
    assert_eq!(report.versions_moved, 4);
    assert!(
        report.folder_removed,
        "the versions did not keep the old folder alive"
    );
    assert!(!old.root.exists());
    let moved = VersionStore::for_project(&new_file).unwrap();
    assert_eq!(moved.entries().len(), 4);
    assert_eq!(moved.dir(), new_root.join("Versions"));
    let named = moved
        .entries()
        .iter()
        .find(|v| v.kind == VersionKind::Named)
        .unwrap();
    assert_eq!(moved.read(named.id).unwrap(), "named text");
}

#[test]
fn a_refused_move_keeps_the_versions_where_they_were() {
    let dir = tmp_dir("reloc-versions-refused");
    let _cleanup = Cleanup(dir.clone());
    let old = old_project(&dir, "Old", &[0.1]);
    keep_versions(&old.file, 2);
    let before = snapshot(&old.root);
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &old.file,
        "old project text",
        Some(&old.root),
        &wanted(&old),
    )
    .unwrap();
    assert!(report.refused.is_some());
    assert_eq!(report.versions_moved, 0);
    assert_eq!(snapshot(&old.root), before);
}

#[test]
fn a_failed_check_removes_no_version() {
    let dir = tmp_dir("reloc-versions-unverified");
    let _cleanup = Cleanup(dir.clone());
    let old = old_project(&dir, "Old", &[0.1]);
    keep_versions(&old.file, 2);
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "Old", "something else");
    let before = snapshot(&old.root);
    let err = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "what was meant",
        Some(&new_root),
        &wanted(&old),
    );
    assert!(err.is_err());
    assert_eq!(
        snapshot(&old.root),
        before,
        "nothing of the old place was removed"
    );
}

#[test]
fn an_unrelated_file_in_the_old_versions_folder_keeps_the_folder() {
    let dir = tmp_dir("reloc-versions-stranger");
    let _cleanup = Cleanup(dir.clone());
    let old = old_project(&dir, "Old", &[0.1]);
    keep_versions(&old.file, 1);
    std::fs::write(old.root.join("Versions").join("mine.txt"), "mine").unwrap();
    let (new_file, new_root) = copy_to(&old, &dir.join("Elsewhere"), "Old", "new text");
    let report = retire_old_project(
        &old.file,
        Some(&old.root),
        &new_file,
        "new text",
        Some(&new_root),
        &wanted(&old),
    )
    .unwrap();
    assert!(!report.folder_removed);
    assert_eq!(
        std::fs::read_to_string(old.root.join("Versions").join("mine.txt")).unwrap(),
        "mine"
    );
}
