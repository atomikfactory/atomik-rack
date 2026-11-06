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
//! that. A folder that also holds anything else stays, with what is in it.
//!
//! Where other project files live in the old folder (or below it) the old audio they use stays:
//! every such file is read, and so are the versions kept beside it ([`crate::versions`]), and
//! only audio that none of them names is removed. If any of them cannot be read (or a folder
//! cannot be listed, or is too deep, or is a link), nothing of the audio is removed and the
//! report says so; only the old project file goes. The versions of the project are copied to the
//! new place, compared byte for byte, and only then removed from the old one.

use crate::bundle::Bundle;
use crate::error::{AudioFileError, Result};
use crate::versions::{
    VERSION_EXTENSION, VersionStore, carry_versions, retire_versions, scan_assets, versions_dir,
};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// How deep below the old folder other project files are looked for; a deeper folder counts as
/// one that could not be read.
const SCAN_DEPTH: usize = 12;
/// The largest project file read to see what audio it uses (the model's own limit).
const MAX_PROJECT_BYTES: u64 = 256 << 20;

/// What [`retire_old_project`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetireReport {
    /// Files removed from the old place (the project file and audio files).
    pub removed_files: usize,
    /// The old project folder is gone.
    pub folder_removed: bool,
    /// Another project lives in the old folder: the audio it uses was left alone.
    pub shared: bool,
    /// Audio files of the old folder that another project uses and that therefore stay.
    pub kept_audio: usize,
    /// A project file (or folder) of the old folder that could not be read: no old audio was
    /// removed. Names what failed.
    pub unreadable: Option<String>,
    /// Versions of the project (see [`crate::versions`]) carried to the new place and removed
    /// from the old one.
    pub versions_moved: usize,
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

/// The other project files of the old folder and what they use.
#[derive(Debug, Default)]
struct OtherProjects {
    /// Project files other than the one being moved.
    files: usize,
    /// Audio any of them (or any version kept beside them) names.
    used: BTreeSet<String>,
    /// What could not be read, if anything.
    unreadable: Option<String>,
}

impl OtherProjects {
    fn fail(&mut self, what: &Path, why: &str) {
        if self.unreadable.is_none() {
            self.unreadable = Some(format!("{}: {why}", what.display()));
        }
    }

    /// Reads the project file `path`: the audio it names and the audio its versions name.
    fn read_project(&mut self, path: &Path) {
        self.files += 1;
        self.read_text(path);
        match VersionStore::for_project(path) {
            Ok(store) => self.used.extend(store.assets()),
            Err(e) => self.fail(path, &format!("its versions: {e}")),
        }
    }

    /// Reads the project or version file `path` and adds the audio it names.
    fn read_text(&mut self, path: &Path) {
        let text = fs::metadata(path).map_err(|e| e.to_string()).and_then(|m| {
            if m.len() > MAX_PROJECT_BYTES {
                Err("too big to read".to_owned())
            } else {
                fs::read_to_string(path).map_err(|e| e.to_string())
            }
        });
        match text {
            Ok(text) => self.used.extend(scan_assets(&text)),
            Err(why) => self.fail(path, &why),
        }
    }
}

/// Reads every project file other than `file` in `dir` and below it, and every version file
/// outside `file`'s own versions folder (a version file the index of its store does not list
/// still names audio a restore would need).
fn scan_other_projects(dir: &Path, file: &Path, depth: usize, out: &mut OtherProjects) {
    let own = fs::canonicalize(file).ok();
    // The moved project's own version files are carried and retired with it.
    let own_versions = versions_dir(file);
    let own_prefix = file
        .file_stem()
        .map(|s| format!("{} v", s.to_string_lossy()));
    let is_own_version = |path: &Path| {
        path.parent() == Some(own_versions.as_path())
            && own_prefix.as_ref().is_some_and(|prefix| {
                path.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(prefix.as_str()))
            })
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return out.fail(dir, &e.to_string()),
    };
    for entry in entries {
        let Ok(entry) = entry else {
            out.fail(dir, "a folder entry cannot be read");
            continue;
        };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            out.fail(&path, "its type cannot be read");
            continue;
        };
        if kind.is_symlink() {
            // A link may lead anywhere (or around): it is not followed, so it is not known.
            out.fail(&path, "a link is not followed");
        } else if kind.is_dir() {
            if depth == 0 {
                out.fail(&path, "too deep to look into");
            } else {
                scan_other_projects(&path, file, depth - 1, out);
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("atomik"))
            && path != file
            && fs::canonicalize(&path).ok() != own
        {
            out.read_project(&path);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(VERSION_EXTENSION))
            && !is_own_version(&path)
        {
            out.read_text(&path);
        }
    }
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

    // The versions go with the project: copied and compared before the old ones are removed.
    let carried = carry_versions(old_file, new_file)?;

    // Every other project of the old folder is read before anything is removed: an old folder
    // that cannot be read fails the move whole, and a project that cannot be read keeps all of
    // the old audio.
    let mut others = OtherProjects::default();
    let old = match old_root {
        Some(root) => {
            let old = Bundle::open(root)?;
            scan_other_projects(root, old_file, SCAN_DEPTH, &mut others);
            Some(old)
        }
        None => None,
    };
    report.shared = others.files > 0;
    report.unreadable = others.unreadable.clone();
    if remove_file_if_present(old_file)? {
        report.removed_files += 1;
    }
    if carried > 0 {
        report.versions_moved = retire_versions(old_file, new_file)?;
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(old) = &old {
        for hash in keep {
            if !old.exists(hash) {
                continue;
            }
            // The only place old audio is removed: never what another project uses, and
            // nothing at all when a project could not be read.
            if others.unreadable.is_some() || others.used.contains(hash) {
                report.kept_audio += 1;
                continue;
            }
            report.removed_files += usize::from(remove_file_if_present(&old.asset_path(hash))?);
            // A cache: a failure to remove it only leaves a stale file.
            let _ = remove_file_if_present(&old.peaks_path(hash));
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
