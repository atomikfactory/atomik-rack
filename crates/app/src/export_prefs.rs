//! The audio export choices the app remembers: `export.json`, next to `settings.json`.
//!
//! ```json
//! { "bit_depth": "24", "dither": "tpdf", "sample_rate": null, "tail": true, "folder": "D:/Music" }
//! ```
//!
//! `bit_depth` is `"16"`, `"24"` or `"32f"`, `dither` is `"none"`, `"tpdf"` or `"shaped"`
//! (it applies to 16-bit files only), `sample_rate` is a rate in Hz or `null` for the project's,
//! `tail` keeps the tail of a song, and `folder` is where the last export went. Loading is
//! lenient: a missing or unreadable file, a missing field or a value this build does not know is
//! the default. Saving is atomic ([`crate::keymap_store::write_atomic`]) and canonical, so an
//! unchanged choice writes identical bytes. These are app preferences, not part of the project.

use atomik_ui::{EXPORT_RATES, ExportDepth, ExportDither, ExportValues};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name.
pub const EXPORT_FILE: &str = "export.json";

/// The export prefs file next to a settings file.
pub fn export_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(EXPORT_FILE)
}

/// What is saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPrefs {
    /// `"16"`, `"24"` or `"32f"`.
    pub bit_depth: String,
    /// `"none"`, `"tpdf"` or `"shaped"`.
    pub dither: String,
    /// The sample rate in Hz, or `None` for the project's.
    pub sample_rate: Option<u32>,
    /// Keep the tail of a song.
    pub tail: bool,
    /// The folder the last export went to.
    pub folder: Option<String>,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        Self::from_values(&ExportValues::default(), None)
    }
}

impl ExportPrefs {
    /// The prefs that say `values` and the last `folder`.
    pub fn from_values(values: &ExportValues, folder: Option<&Path>) -> Self {
        Self {
            bit_depth: match values.depth {
                ExportDepth::Pcm16 => "16",
                ExportDepth::Pcm24 => "24",
                ExportDepth::Float32 => "32f",
            }
            .to_owned(),
            dither: match values.dither {
                ExportDither::Off => "none",
                ExportDither::Tpdf => "tpdf",
                ExportDither::Shaped => "shaped",
            }
            .to_owned(),
            sample_rate: values.rate,
            tail: values.tail,
            folder: folder.map(|f| f.to_string_lossy().into_owned()),
        }
    }

    /// The values the panel starts with: what this build does not know is the default.
    pub fn values(&self) -> ExportValues {
        let default = ExportValues::default();
        ExportValues {
            depth: match self.bit_depth.as_str() {
                "16" => ExportDepth::Pcm16,
                "24" => ExportDepth::Pcm24,
                "32f" => ExportDepth::Float32,
                _ => default.depth,
            },
            dither: match self.dither.as_str() {
                "none" => ExportDither::Off,
                "tpdf" => ExportDither::Tpdf,
                "shaped" => ExportDither::Shaped,
                _ => default.dither,
            },
            rate: self.sample_rate.filter(|r| EXPORT_RATES.contains(r)),
            tail: self.tail,
        }
    }

    /// The folder of the last export.
    pub fn folder(&self) -> Option<PathBuf> {
        self.folder.as_ref().map(PathBuf::from)
    }

    /// Pretty JSON for the file.
    pub fn to_json(&self) -> String {
        // Plain data with string keys: serialization cannot fail.
        let mut json = serde_json::to_string_pretty(self).expect("export prefs serialize");
        json.push('\n');
        json
    }

    /// Reads the prefs at `path`: the defaults when the file is missing or unusable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the prefs to `path`.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::keymap_store::write_atomic(path, &self.to_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("atomik-export-prefs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_defaults_are_24_bit_at_the_project_rate_with_the_tail() {
        let v = ExportPrefs::default().values();
        assert_eq!(v, ExportValues::default());
        assert_eq!(v.depth, ExportDepth::Pcm24);
        assert_eq!((v.rate, v.tail), (None, true));
    }

    #[test]
    fn the_values_round_trip_through_the_file() {
        let d = dir("roundtrip");
        let path = export_path(&d.join("settings.json"));
        assert_eq!(path.file_name().unwrap(), EXPORT_FILE);
        let values = ExportValues {
            depth: ExportDepth::Pcm16,
            dither: ExportDither::Shaped,
            rate: Some(44_100),
            tail: false,
        };
        let prefs = ExportPrefs::from_values(&values, Some(Path::new("music")));
        prefs.save(&path).unwrap();
        let back = ExportPrefs::load(&path);
        assert_eq!(back, prefs);
        assert_eq!(back.values(), values);
        assert_eq!(back.folder(), Some(PathBuf::from("music")));
        // Canonical: saving again writes the same bytes.
        let before = std::fs::read(&path).unwrap();
        back.save(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_or_broken_file_and_unknown_values_are_the_defaults() {
        let d = dir("lenient");
        let path = d.join(EXPORT_FILE);
        assert_eq!(ExportPrefs::load(&path), ExportPrefs::default());
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(ExportPrefs::load(&path), ExportPrefs::default());
        std::fs::write(
            &path,
            r#"{"bit_depth":"20","dither":"fancy","sample_rate":12345,"tail":false}"#,
        )
        .unwrap();
        let v = ExportPrefs::load(&path).values();
        assert_eq!(v.depth, ExportDepth::Pcm24);
        assert_eq!(v.dither, ExportValues::default().dither);
        assert_eq!(v.rate, None);
        assert!(!v.tail);
        // A partial file keeps what it says.
        std::fs::write(&path, r#"{"bit_depth":"16"}"#).unwrap();
        assert_eq!(ExportPrefs::load(&path).values().depth, ExportDepth::Pcm16);
        let _ = std::fs::remove_dir_all(&d);
    }
}
