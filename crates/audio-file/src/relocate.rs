//! Moving a project to a new place: first prove the copy is good, then retire the old one.
//!
//! The caller has already written the new project file and copied the audio the project uses into
//! the new bundle (Save As does both). [`retire_old_project`] then removes what the old place
//! held, and only after every check passed:
//!
//! * the new project file reads back exactly as it was meant to be written;
//! * every asset the project uses that the old bundle holds is in the new bundle, byte for byte
//!   ([`verify_copy`]);
//! * the new place is not the old one, nor inside it, nor around it.
//!
//! What is removed is only what the project owned and a copy now replaces: the old project file
//! and the audio files it used (with their peak caches), then the folders that are empty after
//! that. A folder that also holds anything else stays, with what is in it. Where another project
//! file lives in the old folder (or below it) its `Audio/` store may hold audio that project
//! uses, so the audio is left alone and only the old project file goes.

use crate::bundle::Bundle;
use crate::error::{AudioFileError, Result};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// How deep below the old folder another project file is looked for.
const SCAN_DEPTH: usize = 4;

/// What [`retire_old_project`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetireReport {
    /// Files removed from the old place (the project file and audio files).
    pub removed_files: usize,
    /// The old project folder is gone.
    pub folder_removed: bool,
    /// Another project lives in the old folder: its audio was left alone.
    pub shared: bool,
    /// Why nothing at all was removed (the new place is the old one, or inside it, or around it).
    pub refused: Option<&'static str>,
}

/// Whether the files `a` and `b` hold the same bytes.
fn same_bytes(a: &Path, b: &Path) -> Result<bool> {
    let (ma, mb) = (fs::metadata(a)?, fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (fs::File::open(a)?, fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

/// Checks the copy of the assets `wanted` in `to` against the bundle folder `from`: every asset
/// `from` holds must be in `to` with the same bytes. An asset `from` does not hold is not
/// checked (nothing of it would be removed).
///
/// # Errors
/// [`AudioFileError::Unverified`] naming the first asset that is missing or differs; I/O errors
/// of reading either file.
pub fn verify_copy(from: &Path, to: &Bundle, wanted: &BTreeSet<String>) -> Result<()> {
    let from = Bundle::open(from)?;
    for hash in wanted {
        if !from.exists(hash) {
            continue;
        }
        if !to.exists(hash) {
            return Err(AudioFileError::Unverified(format!(
                "audio file {hash} is not in the new folder"
            )));
        }
        if !same_bytes(&from.asset_path(hash), &to.asset_path(hash))? {
            return Err(AudioFileError::Unverified(format!(
                "audio file {hash} differs from its copy"
            )));
        }
    }
    Ok(())
}

/// Whether `inner` is `outer` or below it, comparing real paths (folders that do not exist are
/// compared as written).
fn within(inner: &Path, outer: &Path) -> bool {
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(inner).starts_with(real(outer))
}

/// Whether a project file other than `file` lives in `dir` or up to [`SCAN_DEPTH`] levels
/// below it.
fn has_other_project(dir: &Path, file: &Path, depth: usize) -> bool {
    let own = fs::canonicalize(file).ok();
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 && has_other_project(&path, file, depth - 1) {
                return true;
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("atomik"))
            && path != file
            && fs::canonicalize(&path).ok() != own
        {
            return true;
        }
    }
    false
}

fn remove_file_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Removes the old project after a move. `old_file` and `old_root` (its bundle folder, when it
/// had one) are the old place; `new_file`, `new_text` (what `new_file` was written from) and
/// `new_root` the new one; `keep` the assets the project uses.
///
/// Nothing is removed unless every check in the module docs passes; a check that fails is an
/// error and leaves the old project exactly as it was. A move that must not remove anything
/// (same place, nested places) is not an error: the report says why in
/// [`RetireReport::refused`].
///
/// # Errors
/// [`AudioFileError::Unverified`] when the new project file or an audio copy does not match;
/// I/O errors (the old project file is removed first, so a failure leaves the audio in place).
pub fn retire_old_project(
    old_file: &Path,
    old_root: Option<&Path>,
    new_file: &Path,
    new_text: &str,
    new_root: Option<&Path>,
    keep: &BTreeSet<String>,
) -> Result<RetireReport> {
    let mut report = RetireReport::default();
    let same = |a: &Path, b: &Path| match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    if same(old_file, new_file) {
        report.refused = Some("the new project file is the old one");
        return Ok(report);
    }
    if let Some(root) = old_root
        && (new_root.is_some_and(|n| within(root, n) || within(n, root)) || within(new_file, root))
    {
        report.refused = Some("the new place is the old folder, or inside it, or around it");
        return Ok(report);
    }

    // Checks first; nothing is removed before all of them passed.
    let written = fs::read_to_string(new_file)
        .map_err(|e| AudioFileError::Unverified(format!("the new project file: {e}")))?;
    if written != new_text {
        return Err(AudioFileError::Unverified(
            "the new project file is not what was written".to_owned(),
        ));
    }
    match (old_root, new_root) {
        (Some(from), Some(to)) => verify_copy(from, &Bundle::open(to)?, keep)?,
        // Old audio with no new folder to hold it: nothing can be verified, so nothing goes.
        (Some(_), None) if !keep.is_empty() => {
            return Err(AudioFileError::Unverified(
                "the audio has no new folder to be checked against".to_owned(),
            ));
        }
        _ => {}
    }

    let shared = old_root.is_some_and(|root| has_other_project(root, old_file, SCAN_DEPTH));
    report.shared = shared;
    // Opened before anything is removed: an old folder that cannot be read fails the move whole.
    let old = match old_root {
        Some(root) if !shared => Some(Bundle::open(root)?),
        _ => None,
    };
    if remove_file_if_present(old_file)? {
        report.removed_files += 1;
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(old) = &old {
        for hash in keep {
            if old.exists(hash) {
                report.removed_files += usize::from(remove_file_if_present(&old.asset_path(hash))?);
                // A cache: a failure to remove it only leaves a stale file.
                let _ = remove_file_if_present(&old.peaks_path(hash));
            }
        }
        dirs.extend([old.peaks_dir(), old.audio_dir(), old.root().to_path_buf()]);
    }
    // `remove_dir` only takes an empty folder: whatever else is there keeps it.
    for dir in &dirs {
        if fs::remove_dir(dir).is_ok() && Some(dir.as_path()) == old_root {
            report.folder_removed = true;
        }
    }
    Ok(report)
}
