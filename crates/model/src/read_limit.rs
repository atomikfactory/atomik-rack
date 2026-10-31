//! Reading a project or patch file with a size cap.
//!
//! A document is parsed into a JSON tree before it is checked, and the tree is several times the
//! size of the text. A file of many gigabytes (corrupt, or made to be opened) would exhaust
//! memory long before any field is validated, and running out of memory aborts the process. So
//! the text is only read when the file is within [`MAX_DOCUMENT_BYTES`], and a larger file is
//! refused with a message naming its size.

use std::io::{self, Read};
use std::path::Path;

/// The largest project or patch file that is read, in bytes: the project's plugin state ceiling
/// ([`crate::MAX_PROJECT_PLUGIN_STATE_BYTES`], 128 MiB, which is a third larger as text) plus
/// room for the rest of the document.
pub const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

/// The environment variable that sets the limit, in MiB, for a user who really has a larger
/// project.
pub const MAX_DOCUMENT_ENV: &str = "ATOMIK_MAX_PROJECT_MIB";

/// The limit in force: [`MAX_DOCUMENT_ENV`] when it holds a whole number of MiB above zero,
/// else [`MAX_DOCUMENT_BYTES`].
pub fn document_size_limit() -> u64 {
    std::env::var(MAX_DOCUMENT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|mib| *mib > 0)
        .map_or(MAX_DOCUMENT_BYTES, |mib| mib.saturating_mul(1024 * 1024))
}

/// Reads a project or patch file to a string, refusing one over [`document_size_limit`] before
/// its text is read. Otherwise as [`std::fs::read_to_string`].
pub fn read_document_text(path: &Path) -> io::Result<String> {
    read_text_limited(path, document_size_limit())
}

/// [`read_document_text`] with an explicit limit in bytes: a file of exactly `max_bytes` is
/// read, one byte more is refused with an `InvalidData` error that names the size.
pub fn read_text_limited(path: &Path, max_bytes: u64) -> io::Result<String> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > max_bytes {
        return Err(too_big(len, max_bytes));
    }
    // The file can grow after the check (and some files report no length): read through a
    // limit that is one byte past the cap, so that overrun is noticed instead of read.
    let mut text = String::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(too_big(text.len() as u64, max_bytes));
    }
    Ok(text)
}

fn too_big(len: u64, max_bytes: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "the file is {} (over the {} limit for project files; set {MAX_DOCUMENT_ENV} to \
             a number of MiB to raise it)",
            size(len),
            size(max_bytes)
        ),
    )
}

/// `len` for a message: whole bytes below 1 KiB, else KiB, MiB or GiB with one decimal.
fn size(len: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)];
    for (name, unit) in UNITS {
        if len >= unit {
            return format!("{:.1} {name}", len as f64 / unit as f64);
        }
    }
    format!("{len} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("atomik-model-read-limit");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{}-{name}", std::process::id()))
    }

    #[test]
    fn a_file_at_the_limit_reads_and_one_byte_more_is_refused() {
        let at = tmp("at.atomik");
        std::fs::write(&at, "x".repeat(1000)).unwrap();
        assert_eq!(read_text_limited(&at, 1000).unwrap().len(), 1000);
        let err = read_text_limited(&at, 999).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("the file is 1000 bytes"), "{err}");
        std::fs::remove_file(at).unwrap();
    }

    #[test]
    fn the_refusal_names_the_size_and_does_not_read_the_text() {
        // A sparse-ish large file: the length alone decides, so nothing is read.
        let big = tmp("big.atomik");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(3 << 30).unwrap();
        drop(file);
        let err = read_document_text(&big).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("3.0 GiB") && text.contains("256.0 MiB"),
            "{text}"
        );
        assert!(text.contains(MAX_DOCUMENT_ENV), "{text}");
        std::fs::remove_file(big).unwrap();
    }

    #[test]
    fn a_missing_file_and_bad_utf8_are_ordinary_errors() {
        assert_eq!(
            read_text_limited(&tmp("nope"), 10).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let bad = tmp("bad.atomik");
        std::fs::write(&bad, [0xFF, 0xFE, 0x00]).unwrap();
        assert_eq!(
            read_text_limited(&bad, 10).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_file(bad).unwrap();
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size(10), "10 bytes");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(256 << 20), "256.0 MiB");
        assert_eq!(size(5 << 30), "5.0 GiB");
    }
}
