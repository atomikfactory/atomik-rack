//! Moving a project: the plan of a collection, cancelling it, verifying a copy and retiring the
//! old project. Nothing may be removed before the copy is proved, and nothing another project
//! uses may be removed at all.
#![allow(missing_docs)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use atomik_audio_file::{
    AudioData, AudioFileError, Bundle, CollectProgress, retire_old_project, verify_copy,
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
    std::fs::write(&other, "other project").unwrap();
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
    assert!(!old.file.exists());
    assert!(other.is_file());
    let b = Bundle::open(&old.root).unwrap();
    assert!(b.exists(&theirs));
    for hash in &old.hashes {
        assert!(b.exists(hash), "shared audio stays where it is");
    }
}

#[test]
fn another_project_in_a_subfolder_also_counts() {
    let dir = tmp_dir("reloc-nested-other");
    let old = old_project(&dir, "Old", &[0.1]);
    std::fs::create_dir_all(old.root.join("Sub").join("Deeper")).unwrap();
    let other = old.root.join("Sub").join("Deeper").join("Other.atomik");
    std::fs::write(&other, "other project").unwrap();
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
