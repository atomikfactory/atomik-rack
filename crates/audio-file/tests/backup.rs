//! Backup copies: the copy and its audio, de-duplication, pruning that never removes audio a kept
//! copy names, and the list of copies.
#![allow(missing_docs)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use atomik_audio_file::{AudioData, BackupPlan, Bundle, backup_project, list_backups, scan_assets};

/// A scratch folder that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atomik-backup-tests-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn recording(seed: f32) -> AudioData {
    let l: Vec<f32> = (0..4000)
        .map(|i| ((i as f32) * 0.01 + seed).sin() * 0.5)
        .collect();
    AudioData::new(48_000, vec![l]).unwrap()
}

/// A project `Song` in `dir/Song` with one audio file per seed; returns its file, text, bundle
/// and the hashes.
struct Project {
    file: PathBuf,
    text: String,
    bundle: Bundle,
    hashes: Vec<String>,
}

fn project(dir: &Path, seeds: &[f32]) -> Project {
    let root = dir.join("Song");
    let mut bundle = Bundle::create(&root).unwrap();
    let hashes: Vec<String> = seeds
        .iter()
        .map(|s| bundle.add_recording(&recording(*s), "x").unwrap().hash)
        .collect();
    let text = text_of(&hashes);
    let file = root.join("Song.atomik");
    std::fs::write(&file, &text).unwrap();
    Project {
        file,
        text,
        bundle,
        hashes,
    }
}

fn text_of(hashes: &[String]) -> String {
    let list: Vec<String> = hashes.iter().map(|h| format!("\"{h}\"")).collect();
    format!("project [{}]", list.join(", "))
}

fn plan<'a>(folder: &'a Path, stamp: &'a str, keep: usize, audio: bool) -> BackupPlan<'a> {
    BackupPlan {
        folder,
        name: "Song",
        stamp,
        keep,
        include_audio: audio,
    }
}

fn audio_files(folder: &Path) -> BTreeSet<String> {
    let dir = folder.join("Song").join("Audio");
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_backup_writes_the_copy_and_the_audio_it_names() {
    let tmp = Scratch::new("copy");
    let p = project(tmp.path(), &[1.0, 2.0]);
    let backup = tmp.path().join("Backups");
    let r = backup_project(
        &plan(&backup, "2026-11-10 093112", 10, true),
        &p.file,
        &p.text,
        Some(&p.bundle),
    )
    .unwrap();
    assert_eq!(
        r.copy,
        backup.join("Song").join("Song 2026-11-10 093112.atomik")
    );
    assert_eq!(std::fs::read_to_string(&r.copy).unwrap(), p.text);
    assert_eq!(
        (r.audio_copied, r.audio_present, r.audio_missing),
        (2, 0, 0)
    );
    let want: BTreeSet<String> = p.hashes.iter().map(|h| format!("{h}.wav")).collect();
    assert_eq!(audio_files(&backup), want);
    // The audio is the same bytes.
    for h in &p.hashes {
        assert_eq!(
            std::fs::read(p.bundle.asset_path(h)).unwrap(),
            std::fs::read(backup.join("Song/Audio").join(format!("{h}.wav"))).unwrap()
        );
    }
    // No temporary file is left.
    let names: Vec<String> = std::fs::read_dir(backup.join("Song"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with("tmp~")), "{names:?}");
}

#[test]
fn without_audio_only_the_copy_is_written() {
    let tmp = Scratch::new("no-audio");
    let p = project(tmp.path(), &[1.0]);
    let backup = tmp.path().join("Backups");
    let r = backup_project(
        &plan(&backup, "2026-11-10 093112", 10, false),
        &p.file,
        &p.text,
        Some(&p.bundle),
    )
    .unwrap();
    assert!(r.copy.is_file());
    assert_eq!(r.audio_copied, 0);
    assert!(audio_files(&backup).is_empty());
}

#[test]
fn audio_is_copied_once_across_saves() {
    let tmp = Scratch::new("dedup");
    let p = project(tmp.path(), &[1.0, 2.0]);
    let backup = tmp.path().join("Backups");
    let first = backup_project(
        &plan(&backup, "2026-11-10 093112", 10, true),
        &p.file,
        &p.text,
        Some(&p.bundle),
    )
    .unwrap();
    assert_eq!(first.audio_copied, 2);
    // A file that is already there is the one wanted: it is not written again.
    let marker = backup
        .join("Song/Audio")
        .join(format!("{}.wav", p.hashes[0]));
    let before = std::fs::metadata(&marker).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let second = backup_project(
        &plan(&backup, "2026-11-10 093200", 10, true),
        &p.file,
        &p.text,
        Some(&p.bundle),
    )
    .unwrap();
    assert_eq!((second.audio_copied, second.audio_present), (0, 2));
    assert_eq!(
        std::fs::metadata(&marker).unwrap().modified().unwrap(),
        before
    );
    assert_eq!(list_backups(&backup, "Song").unwrap().len(), 2);
}

#[test]
fn audio_the_project_folder_lacks_is_counted_and_never_fails_the_backup() {
    let tmp = Scratch::new("missing");
    let p = project(tmp.path(), &[1.0]);
    let ghost = "a".repeat(64);
    let text = text_of(&[p.hashes[0].clone(), ghost]);
    std::fs::write(&p.file, &text).unwrap();
    let backup = tmp.path().join("Backups");
    let r = backup_project(
        &plan(&backup, "2026-11-10 093112", 10, true),
        &p.file,
        &text,
        Some(&p.bundle),
    )
    .unwrap();
    assert_eq!((r.audio_copied, r.audio_missing), (1, 1));
    assert!(r.copy.is_file());
}

#[test]
fn pruning_keeps_the_newest_copies_and_the_audio_they_name() {
    let tmp = Scratch::new("prune");
    let backup = tmp.path().join("Backups");
    let p = project(tmp.path(), &[1.0, 2.0, 3.0]);
    // Three saves: the song uses audio 0, then 0+1, then 1+2 (audio 0 is gone from the last).
    let versions: [(&str, Vec<String>); 3] = [
        ("2026-11-10 090000", vec![p.hashes[0].clone()]),
        (
            "2026-11-10 100000",
            vec![p.hashes[0].clone(), p.hashes[1].clone()],
        ),
        (
            "2026-11-10 110000",
            vec![p.hashes[1].clone(), p.hashes[2].clone()],
        ),
    ];
    let mut last = None;
    for (stamp, hashes) in &versions {
        let text = text_of(hashes);
        std::fs::write(&p.file, &text).unwrap();
        last = Some(
            backup_project(
                &plan(&backup, stamp, 2, true),
                &p.file,
                &text,
                Some(&p.bundle),
            )
            .unwrap(),
        );
    }
    let last = last.unwrap();
    let stamps: Vec<String> = list_backups(&backup, "Song")
        .unwrap()
        .into_iter()
        .map(|e| e.stamp)
        .collect();
    assert_eq!(stamps, vec!["2026-11-10 110000", "2026-11-10 100000"]);
    // The kept copies name audio 0 (the middle one), 1 and 2: all of it stays.
    let all: BTreeSet<String> = p.hashes.iter().map(|h| format!("{h}.wav")).collect();
    assert_eq!(audio_files(&backup), all);
    assert_eq!(last.pruned, 1);
    assert_eq!(last.audio_removed, 0);
    // A fourth save pushes the middle copy out: audio 0 is named by nothing and goes.
    let text = text_of(&[p.hashes[2].clone()]);
    std::fs::write(&p.file, &text).unwrap();
    let r = backup_project(
        &plan(&backup, "2026-11-10 120000", 2, true),
        &p.file,
        &text,
        Some(&p.bundle),
    )
    .unwrap();
    assert_eq!((r.pruned, r.audio_removed), (1, 1));
    let rest: BTreeSet<String> = [&p.hashes[1], &p.hashes[2]]
        .iter()
        .map(|h| format!("{h}.wav"))
        .collect();
    assert_eq!(audio_files(&backup), rest);
    for entry in list_backups(&backup, "Song").unwrap() {
        for hash in scan_assets(&std::fs::read_to_string(&entry.path).unwrap()) {
            assert!(
                backup
                    .join("Song/Audio")
                    .join(format!("{hash}.wav"))
                    .is_file()
            );
        }
    }
}

#[test]
fn an_unreadable_project_file_keeps_all_the_audio() {
    let tmp = Scratch::new("unreadable");
    let backup = tmp.path().join("Backups");
    let p = project(tmp.path(), &[1.0, 2.0]);
    backup_project(
        &plan(&backup, "2026-11-10 090000", 5, true),
        &p.file,
        &p.text,
        Some(&p.bundle),
    )
    .unwrap();
    // A project file of the folder that is not text: what it names is not known.
    std::fs::write(backup.join("Song").join("Other.atomik"), [0xff, 0xfe, 0xfd]).unwrap();
    let text = text_of(&[p.hashes[0].clone()]);
    std::fs::write(&p.file, &text).unwrap();
    let r = backup_project(
        &plan(&backup, "2026-11-10 100000", 1, true),
        &p.file,
        &text,
        Some(&p.bundle),
    )
    .unwrap();
    assert_eq!(r.pruned, 1);
    assert_eq!(r.audio_removed, 0);
    assert!(r.unreadable.is_some());
    assert_eq!(audio_files(&backup).len(), 2);
    // Files that are not copies stay.
    assert!(backup.join("Song").join("Other.atomik").is_file());
}

#[test]
fn files_that_are_not_copies_are_never_removed() {
    let tmp = Scratch::new("others");
    let backup = tmp.path().join("Backups");
    let p = project(tmp.path(), &[1.0]);
    let dir = backup.join("Song");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    std::fs::write(dir.join("Song final.atomik"), &p.text).unwrap();
    for stamp in ["2026-11-10 090000", "2026-11-10 100000"] {
        backup_project(
            &plan(&backup, stamp, 1, true),
            &p.file,
            &p.text,
            Some(&p.bundle),
        )
        .unwrap();
    }
    assert!(dir.join("notes.txt").is_file());
    assert!(dir.join("Song final.atomik").is_file());
    assert_eq!(list_backups(&backup, "Song").unwrap().len(), 1);
}

#[test]
fn the_list_is_newest_first_and_empty_without_a_folder() {
    let tmp = Scratch::new("list");
    let backup = tmp.path().join("Backups");
    assert!(list_backups(&backup, "Song").unwrap().is_empty());
    let p = project(tmp.path(), &[]);
    for stamp in [
        "2026-11-10 090000",
        "2026-11-12 080000",
        "2026-11-10 235959",
    ] {
        backup_project(&plan(&backup, stamp, 10, true), &p.file, &p.text, None).unwrap();
    }
    let list = list_backups(&backup, "Song").unwrap();
    let stamps: Vec<&str> = list.iter().map(|e| e.stamp.as_str()).collect();
    assert_eq!(
        stamps,
        [
            "2026-11-12 080000",
            "2026-11-10 235959",
            "2026-11-10 090000"
        ]
    );
    assert!(list.iter().all(|e| e.size == p.text.len() as u64));
}

#[test]
fn a_bad_name_or_stamp_is_refused() {
    let tmp = Scratch::new("bad");
    let p = project(tmp.path(), &[]);
    let backup = tmp.path().join("Backups");
    let bad_name = BackupPlan {
        name: "../Song",
        ..plan(&backup, "2026-11-10 093112", 3, true)
    };
    assert!(backup_project(&bad_name, &p.file, &p.text, None).is_err());
    let bad_stamp = plan(&backup, "yesterday", 3, true);
    assert!(backup_project(&bad_stamp, &p.file, &p.text, None).is_err());
    assert!(!backup.exists());
}

#[test]
fn a_folder_that_cannot_be_made_is_an_error() {
    let tmp = Scratch::new("blocked");
    let p = project(tmp.path(), &[]);
    // A file where the backup folder should be.
    let blocked = tmp.path().join("Backups");
    std::fs::write(&blocked, "a file").unwrap();
    let r = backup_project(
        &plan(&blocked, "2026-11-10 093112", 3, true),
        &p.file,
        &p.text,
        None,
    );
    assert!(r.is_err());
}
