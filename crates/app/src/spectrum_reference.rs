//! The stored reference curve of the spectrum analyser: 31 one-third-octave band levels in dB,
//! captured from a signal and kept in `spectrum-reference.json` next to the library file, so
//! every spectrum device of every project can compare against it.
//!
//! ```text
//! { "version": 1, "bands": [ -42.5, -41.0, ... 31 numbers ... ] }
//! ```
//!
//! A missing file means no curve. A damaged one is ignored (with a log line) and replaced by the
//! next capture; a file from a newer version is left alone and never overwritten.

use atomik_dsp::spectrum::{BAND_COUNT, FLOOR_DB};
use std::path::{Path, PathBuf};

/// The file name of the stored curve.
pub const REFERENCE_FILE: &str = "spectrum-reference.json";
/// The format version this build writes and reads.
const VERSION: u64 = 1;
/// The loudest level a stored band may hold, dB.
const MAX_DB: f32 = 40.0;

/// The curve file next to a library file.
pub fn reference_path(library_file: &Path) -> PathBuf {
    library_file.with_file_name(REFERENCE_FILE)
}

/// Why a curve file was not used.
#[derive(Debug, thiserror::Error)]
pub enum ReferenceError {
    /// The text is not a curve this build understands.
    #[error("{0}")]
    Invalid(String),
    /// Written by a newer version.
    #[error("written by a newer version ({0})")]
    Newer(u64),
}

/// The curve in its file form.
pub fn encode(bands: &[f32; BAND_COUNT]) -> String {
    let values: Vec<String> = bands.iter().map(|v| format!("{v:.2}")).collect();
    format!(
        "{{\n  \"version\": {VERSION},\n  \"bands\": [{}]\n}}\n",
        values.join(", ")
    )
}

/// Reads the file form.
///
/// # Errors
/// [`ReferenceError::Invalid`] for anything but a version-1 object with 31 finite numbers,
/// [`ReferenceError::Newer`] for a later version.
pub fn parse(text: &str) -> Result<[f32; BAND_COUNT], ReferenceError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ReferenceError::Invalid(e.to_string()))?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| ReferenceError::Invalid("no version".to_string()))?;
    if version > VERSION {
        return Err(ReferenceError::Newer(version));
    }
    let list = value
        .get("bands")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ReferenceError::Invalid("no band list".to_string()))?;
    if list.len() != BAND_COUNT {
        return Err(ReferenceError::Invalid(format!(
            "{} bands instead of {BAND_COUNT}",
            list.len()
        )));
    }
    let mut bands = [FLOOR_DB; BAND_COUNT];
    for (slot, v) in bands.iter_mut().zip(list) {
        let x = v
            .as_f64()
            .filter(|x| x.is_finite())
            .ok_or_else(|| ReferenceError::Invalid("a band is not a number".to_string()))?;
        *slot = (x as f32).clamp(FLOOR_DB, MAX_DB);
    }
    Ok(bands)
}

/// What [`load`] found.
#[derive(Debug, PartialEq)]
pub enum Loaded {
    /// A stored curve.
    Curve([f32; BAND_COUNT]),
    /// No file.
    Missing,
    /// A file that cannot be used (damaged or from a newer version): the curve is empty and,
    /// for a newer version, the file must not be overwritten.
    Unusable {
        /// Why.
        why: String,
        /// A newer version wrote it.
        newer: bool,
    },
}

/// Reads the curve at `path`.
pub fn load(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(bands) => Loaded::Curve(bands),
            Err(e @ ReferenceError::Newer(_)) => Loaded::Unusable {
                why: e.to_string(),
                newer: true,
            },
            Err(e) => Loaded::Unusable {
                why: e.to_string(),
                newer: false,
            },
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Unusable {
            why: e.to_string(),
            newer: true,
        },
    }
}

/// Writes the curve atomically.
///
/// # Errors
/// The I/O error.
pub fn save(path: &Path, bands: &[f32; BAND_COUNT]) -> std::io::Result<()> {
    crate::recovery::atomic_write(path, encode(bands).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> [f32; BAND_COUNT] {
        std::array::from_fn(|i| -30.0 - i as f32 * 1.5)
    }

    #[test]
    fn a_curve_round_trips_through_its_file_form() {
        let bands = curve();
        let back = parse(&encode(&bands)).unwrap();
        for (a, b) in bands.iter().zip(&back) {
            assert!((a - b).abs() < 0.006, "{a} {b}");
        }
    }

    #[test]
    fn only_a_version_one_list_of_31_finite_numbers_is_a_curve() {
        for bad in [
            "",
            "[]",
            "{}",
            "{\"version\":1}",
            "{\"version\":1,\"bands\":[1,2,3]}",
            "{\"version\":1,\"bands\":\"x\"}",
        ] {
            assert!(
                matches!(parse(bad), Err(ReferenceError::Invalid(_))),
                "{bad}"
            );
        }
        let mut list: Vec<String> = vec!["-20".to_string(); BAND_COUNT];
        list[4] = "\"loud\"".to_string();
        let text = format!("{{\"version\":1,\"bands\":[{}]}}", list.join(","));
        assert!(matches!(parse(&text), Err(ReferenceError::Invalid(_))));
        let newer = "{\"version\":2,\"bands\":[]}";
        assert!(matches!(parse(newer), Err(ReferenceError::Newer(2))));
    }

    #[test]
    fn levels_are_clamped_to_what_a_spectrum_can_show() {
        let list: Vec<String> = (0..BAND_COUNT)
            .map(|i| if i == 0 { "900" } else { "-900" }.to_string())
            .collect();
        let text = format!("{{\"version\":1,\"bands\":[{}]}}", list.join(","));
        let bands = parse(&text).unwrap();
        assert!((bands[0] - MAX_DB).abs() < 1e-6);
        assert!((bands[1] - FLOOR_DB).abs() < 1e-6);
    }

    #[test]
    fn the_file_lives_next_to_the_library() {
        let p = reference_path(Path::new("cfg/library.json"));
        assert_eq!(p, Path::new("cfg").join(REFERENCE_FILE));
    }

    #[test]
    fn loading_tells_missing_from_damaged_from_newer() {
        let dir = std::env::temp_dir().join(format!("atomik-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(REFERENCE_FILE);
        let _ = std::fs::remove_file(&file);
        assert_eq!(load(&file), Loaded::Missing);
        save(&file, &curve()).unwrap();
        assert!(matches!(load(&file), Loaded::Curve(_)));
        assert!(!dir.join("spectrum-reference.json.tmp").exists());
        std::fs::write(&file, "not json").unwrap();
        assert!(matches!(load(&file), Loaded::Unusable { newer: false, .. }));
        std::fs::write(&file, "{\"version\":9,\"bands\":[]}").unwrap();
        assert!(matches!(load(&file), Loaded::Unusable { newer: true, .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
