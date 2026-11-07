//! Copying a file from outside the project into the project's own folder.
//!
//! Collect Files uses [`collect_external_file`] for the preset and impulse files a plugin names:
//! the file is copied into a folder of the project (never over a file that is there), and the
//! caller points the project at the copy. Nothing outside the project folder is written, changed
//! or removed.

use crate::bundle::write_atomic;
use crate::error::{AudioFileError, Result};
use crate::relocate::same_bytes;
use std::fs;
use std::path::{Path, PathBuf};

/// The largest file copied; a bigger one is listed as not collected.
pub const MAX_EXTERNAL_BYTES: u64 = 64 << 20;
/// How many names `name 2`, `name 3`... are tried before the copy gives up.
const MAX_SAME_NAMES: u32 = 99;

/// What [`collect_external_file`] did with one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalCopy {
    /// Copied: the project's copy is `dest`.
    Copied {
        /// The copy.
        dest: PathBuf,
        /// Its size in bytes.
        bytes: u64,
    },
    /// The folder already holds the file (the file is inside it, or a file of the same bytes is
    /// there): nothing was written; the project's file is `dest`.
    Present {
        /// The file the project should name.
        dest: PathBuf,
    },
    /// There is no such file.
    Missing,
    /// The file is left where it is, and why.
    Refused(&'static str),
}

/// Whether `inner` is `outer` or below it, comparing real paths.
fn is_inside(inner: &Path, outer: &Path) -> bool {
    match (fs::canonicalize(inner), fs::canonicalize(outer)) {
        (Ok(inner), Ok(outer)) => inner.starts_with(outer),
        _ => false,
    }
}

/// The name `name`, `name 2`, `name 3`... with the extension kept.
fn numbered(name: &str, n: u32) -> String {
    if n == 1 {
        return name.to_owned();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} {n}.{ext}"),
        _ => format!("{name} {n}"),
    }
}

/// Copies the file `src` into the folder `dest_dir` (made when needed) under its own name; where
/// that name is taken by a file with other bytes, under `name 2`, `name 3`... A file that is
/// already inside `dest_dir`, or whose bytes the folder holds under its name, is not copied.
/// Nothing in `dest_dir` is overwritten and `src` is never changed.
///
/// # Errors
/// I/O errors of reading `src` or writing the copy (no partial file is left), and
/// [`AudioFileError::InvalidData`] when 99 names are taken.
pub fn collect_external_file(src: &Path, dest_dir: &Path) -> Result<ExternalCopy> {
    let Ok(meta) = fs::metadata(src) else {
        return Ok(ExternalCopy::Missing);
    };
    if !meta.is_file() {
        return Ok(ExternalCopy::Missing);
    }
    if is_inside(src, dest_dir) {
        return Ok(ExternalCopy::Present {
            dest: src.to_path_buf(),
        });
    }
    if meta.len() > MAX_EXTERNAL_BYTES {
        return Ok(ExternalCopy::Refused("it is over 64 MiB"));
    }
    let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(ExternalCopy::Missing);
    };
    fs::create_dir_all(dest_dir)?;
    for n in 1..=MAX_SAME_NAMES {
        let dest = dest_dir.join(numbered(&name, n));
        if !dest.exists() {
            write_atomic(&dest, |tmp| {
                fs::copy(src, tmp)?;
                Ok(())
            })?;
            return Ok(ExternalCopy::Copied {
                dest,
                bytes: meta.len(),
            });
        }
        if dest.is_file() && same_bytes(src, &dest)? {
            return Ok(ExternalCopy::Present { dest });
        }
    }
    Err(AudioFileError::InvalidData(format!(
        "{MAX_SAME_NAMES} files named like {name} are in {}",
        dest_dir.display()
    )))
}
