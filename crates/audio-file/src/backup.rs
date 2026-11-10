//! Backup copies of a project in a second folder (another drive, a synced folder).
//!
//! A project `Song` backed up into the folder `B` has this layout:
//!
//! ```text
//! B/Song/Song 2026-11-10 093112.atomik     one copy per save (the time is UTC)
//! B/Song/Song 2026-11-10 101500.atomik
//! B/Song/Audio/<hash>.wav                  every audio file a kept copy uses, once
//! ```
//!
//! Audio files are named by the hash of their samples, so a file that is already in `Audio/` is
//! the one wanted and is never copied again. [`backup_project`] writes the copy and the audio the
//! copy needs, then prunes: only the newest `keep` copies stay, and an audio file stays as long as
//! any copy still there names it (mark and sweep, as Move Project does for an old folder: every
//! project file of the folder is read first, and one that cannot be read keeps all the audio).
//! Files that are not a copy are never removed. Nothing here touches the project being backed up.
//!
//! [`list_backups`] lists the copies of a project, newest first, for the restore list.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::is_safe_component;
use crate::bundle::{Bundle, is_valid_hash, write_atomic};
use crate::error::{AudioFileError, Result};
use crate::versions::scan_assets;

/// The extension of a backup copy (the same as a project file).
pub const BACKUP_EXTENSION: &str = "atomik";
/// The most copies a project may keep.
pub const MAX_BACKUPS: usize = 50;
/// The length of a time stamp in a copy's name: `2026-11-10 093112`.
const STAMP_LEN: usize = 17;
/// The largest copy read to see what audio it names.
const MAX_COPY_BYTES: u64 = 256 << 20;

/// Where a project's copies are: `<folder>/<name>`.
pub fn backup_dir(folder: &Path, name: &str) -> PathBuf {
    folder.join(name)
}

/// What a backup is made of.
#[derive(Debug, Clone)]
pub struct BackupPlan<'a> {
    /// The backup folder the user chose.
    pub folder: &'a Path,
    /// The project's name (its file name without the extension).
    pub name: &'a str,
    /// The time stamp of the copy, `YYYY-MM-DD HHMMSS`.
    pub stamp: &'a str,
    /// How many copies stay (1 to [`MAX_BACKUPS`]).
    pub keep: usize,
    /// Whether the audio the project uses is copied too.
    pub include_audio: bool,
}

/// What [`backup_project`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupReport {
    /// The copy that was written.
    pub copy: PathBuf,
    /// Audio files copied this time.
    pub audio_copied: usize,
    /// Audio files the backup already held.
    pub audio_present: usize,
    /// Audio files the project names that its own folder did not hold.
    pub audio_missing: usize,
    /// Old copies removed.
    pub pruned: usize,
    /// Audio files no copy names any more, removed.
    pub audio_removed: usize,
    /// A copy that could not be read: no audio was removed.
    pub unreadable: Option<String>,
}

/// One copy of a project, for the restore list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    /// The copy's file.
    pub path: PathBuf,
    /// When it was made: `2026-11-10 093112` (UTC).
    pub stamp: String,
    /// Its size in bytes.
    pub size: u64,
}

/// `true` for `YYYY-MM-DD HHMMSS`.
fn is_stamp(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == STAMP_LEN
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b' ',
            _ => c.is_ascii_digit(),
        })
}

/// The time stamp in the name of the copy `file_name` of the project `name`, when it is one:
/// `<name> <stamp>.atomik`.
fn stamp_of(name: &str, file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix(name)?.strip_prefix(' ')?;
    let stamp = rest.strip_suffix(&format!(".{BACKUP_EXTENSION}"))?;
    is_stamp(stamp).then(|| stamp.to_owned())
}

/// The copies of the project `name` in `folder`, newest first. A folder that is not there lists
/// nothing.
///
/// # Errors
/// The folder exists but cannot be read.
pub fn list_backups(folder: &Path, name: &str) -> Result<Vec<BackupEntry>> {
    let dir = backup_dir(folder, name);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(stamp) = stamp_of(name, &file_name) else {
            continue;
        };
        let meta = entry.metadata()?;
        if meta.is_file() {
            out.push(BackupEntry {
                path: entry.path(),
                stamp,
                size: meta.len(),
            });
        }
    }
    out.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(out)
}

/// Copies `from` to `to` through a temporary file, so a copy that fails never leaves a partial
/// file under its real name.
fn copy_atomic(from: &Path, to: &Path) -> Result<()> {
    write_atomic(to, |tmp| {
        fs::copy(from, tmp)?;
        Ok(())
    })
}

/// The audio hashes the project files of `dir` name, and a note of the first file that could not
/// be read (every `.atomik` file counts, not only the stamped copies).
fn marked_audio(dir: &Path) -> (BTreeSet<String>, Option<String>) {
    let mut used = BTreeSet::new();
    let mut unreadable = None;
    let mut fail = |what: &Path, why: String| {
        unreadable.get_or_insert_with(|| format!("{}: {why}", what.display()));
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            fail(dir, e.to_string());
            return (used, unreadable);
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            fail(dir, "a folder entry cannot be read".to_owned());
            continue;
        };
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(BACKUP_EXTENSION))
        {
            continue;
        }
        let text = fs::metadata(&path)
            .map_err(|e| e.to_string())
            .and_then(|m| {
                if m.len() > MAX_COPY_BYTES {
                    Err("too big to read".to_owned())
                } else {
                    fs::read_to_string(&path).map_err(|e| e.to_string())
                }
            });
        match text {
            Ok(text) => used.extend(scan_assets(&text)),
            Err(why) => fail(&path, why),
        }
    }
    (used, unreadable)
}

/// Removes the audio files of `audio_dir` that no copy names. Returns how many went.
fn sweep_audio(audio_dir: &Path, used: &BTreeSet<String>) -> usize {
    let Ok(entries) = fs::read_dir(audio_dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(hash) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".wav"))
        else {
            continue;
        };
        if is_valid_hash(hash)
            && !used.contains(hash)
            && entry.file_type().is_ok_and(|t| t.is_file())
            && fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// Backs up the saved project file `project`: copies it into the backup folder, with the audio
/// it uses taken from `source` (the project's own bundle) when the plan includes audio, then
/// prunes (module docs). `text` is the project text, which names the audio.
///
/// A copy of the same time stamp is replaced. The project and its bundle are only read.
///
/// # Errors
/// The project name is not a plain file name; the backup folder cannot be made or written; the
/// project file cannot be read. Audio that `source` does not hold is counted in the report, not
/// an error; a failure while pruning is not an error either (the next backup prunes again).
pub fn backup_project(
    plan: &BackupPlan<'_>,
    project: &Path,
    text: &str,
    source: Option<&Bundle>,
) -> Result<BackupReport> {
    if !is_safe_component(plan.name) || !is_stamp(plan.stamp) {
        return Err(AudioFileError::InvalidData(format!(
            "cannot back up a project named {:?}",
            plan.name
        )));
    }
    let dir = backup_dir(plan.folder, plan.name);
    let dest = Bundle::create(&dir)?;
    let mut report = BackupReport::default();
    // The audio first, so that a copy is never in the folder without the audio it names.
    if plan.include_audio {
        for hash in scan_assets(text) {
            if dest.exists(&hash) {
                report.audio_present += 1;
            } else if let Some(from) = source.filter(|b| b.exists(&hash)) {
                match copy_atomic(&from.asset_path(&hash), &dest.asset_path(&hash)) {
                    Ok(()) => report.audio_copied += 1,
                    // A save that ran meanwhile removed the file: it is not used any more.
                    Err(AudioFileError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                        report.audio_missing += 1;
                    }
                    Err(e) => return Err(e),
                }
            } else {
                report.audio_missing += 1;
            }
        }
    }
    report.copy = dir.join(format!("{} {}.{BACKUP_EXTENSION}", plan.name, plan.stamp));
    copy_atomic(project, &report.copy)?;
    prune(&dir, plan, &mut report);
    Ok(report)
}

/// Keeps the newest copies and sweeps the audio they do not name.
fn prune(dir: &Path, plan: &BackupPlan<'_>, report: &mut BackupReport) {
    let keep = plan.keep.clamp(1, MAX_BACKUPS);
    if let Ok(copies) = list_backups(plan.folder, plan.name) {
        for old in copies.into_iter().skip(keep) {
            if fs::remove_file(&old.path).is_ok() {
                report.pruned += 1;
            }
        }
    }
    // What stays decides what audio stays: all of it when a file cannot be read.
    let (used, unreadable) = marked_audio(dir);
    report.unreadable = unreadable;
    if report.unreadable.is_none() {
        report.audio_removed = sweep_audio(&dir.join("Audio"), &used);
    }
}
