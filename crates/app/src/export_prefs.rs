//! The audio export choices the app remembers: `export.json`, next to `settings.json`.
//!
//! ```json
//! { "bit_depth": "24", "dither": "tpdf", "sample_rate": null, "tail": true, "file": "wav",
//!   "dry": false, "locators": false, "mp3": "cbr192", "aac_kbps": 192, "opus_kbps": 128,
//!   "folder": "D:/Music" }
//! ```
//!
//! `bit_depth` is `"16"`, `"24"` or `"32f"`, `dither` is `"none"`, `"tpdf"` or `"shaped"`
//! (it applies to 16-bit files only), `sample_rate` is a rate in Hz or `null` for the project's,
//! `tail` keeps the tail of a song, `file` is the container (`"wav"`, `"aiff"`, `"flac"`, `"mp3"`,
//! `"aac"` or `"opus"`; FLAC has no `"32f"`), `mp3` is the MP3 quality (`"cbr128"`, `"cbr192"`,
//! `"cbr256"`, `"cbr320"`, `"v0"`, `"v2"` or `"v4"`), `aac_kbps` and `opus_kbps` are the bitrates
//! of those formats in kilobits per second, `dry` takes the channels of a stems export before their strips, `locators`
//! exports stems only between the locators, and `folder` is where the last export went. Loading
//! is
//! lenient: a missing or unreadable file, a missing field or a value this build does not know is
//! the default. Saving is atomic ([`crate::keymap_store::write_atomic`]) and canonical, so an
//! unchanged choice writes identical bytes. These are app preferences, not part of the project.

use atomik_ui::{
    AAC_BITRATES, EXPORT_RATES, ExportDepth, ExportDither, ExportFile, ExportQuality, ExportValues,
    MP3_BITRATES, MP3_VBR, OPUS_BITRATES,
};
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
    /// `"wav"`, `"aiff"`, `"flac"`, `"mp3"`, `"aac"` or `"opus"`.
    pub file: String,
    /// The MP3 quality: `"cbr192"`, `"v2"`, and so on.
    pub mp3: String,
    /// The AAC bitrate in kilobits per second.
    pub aac_kbps: u16,
    /// The Opus bitrate in kilobits per second.
    pub opus_kbps: u16,
    /// Stems only: the channels before their strips.
    pub dry: bool,
    /// Stems only: just the range between the locators.
    pub locators: bool,
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
            file: match values.file {
                ExportFile::Wav => "wav",
                ExportFile::Aiff => "aiff",
                ExportFile::Flac => "flac",
                ExportFile::Mp3 => "mp3",
                ExportFile::Aac => "aac",
                ExportFile::Opus => "opus",
            }
            .to_owned(),
            mp3: match values.mp3 {
                ExportQuality::Cbr(kbps) => format!("cbr{kbps}"),
                ExportQuality::Vbr(q) => format!("v{q}"),
            },
            aac_kbps: values.aac,
            opus_kbps: values.opus,
            dry: values.dry,
            locators: values.locators,
            folder: folder.map(|f| f.to_string_lossy().into_owned()),
        }
    }

    /// The values the panel starts with: what this build does not know is the default.
    pub fn values(&self) -> ExportValues {
        let default = ExportValues::default();
        let file = match self.file.as_str() {
            "wav" => ExportFile::Wav,
            "aiff" => ExportFile::Aiff,
            "flac" => ExportFile::Flac,
            "mp3" => ExportFile::Mp3,
            "aac" => ExportFile::Aac,
            "opus" => ExportFile::Opus,
            _ => default.file,
        };
        ExportValues {
            depth: match self.bit_depth.as_str() {
                "16" => ExportDepth::Pcm16,
                "24" => ExportDepth::Pcm24,
                // FLAC has no float: a file that says both gets 24-bit.
                "32f" if file != ExportFile::Flac => ExportDepth::Float32,
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
            file,
            dry: self.dry,
            locators: self.locators,
            mp3: self.mp3_quality().unwrap_or(default.mp3),
            aac: Some(self.aac_kbps)
                .filter(|k| AAC_BITRATES.contains(k))
                .unwrap_or(default.aac),
            opus: Some(self.opus_kbps)
                .filter(|k| OPUS_BITRATES.contains(k))
                .unwrap_or(default.opus),
            // The time selection is not remembered: it belongs to the song that is open.
            selection: false,
        }
    }

    /// The MP3 quality the text names, when this build offers it.
    fn mp3_quality(&self) -> Option<ExportQuality> {
        let text = self.mp3.as_str();
        if let Some(kbps) = text.strip_prefix("cbr").and_then(|k| k.parse::<u16>().ok()) {
            return MP3_BITRATES
                .contains(&kbps)
                .then_some(ExportQuality::Cbr(kbps));
        }
        let q = text.strip_prefix('v').and_then(|q| q.parse::<u8>().ok())?;
        MP3_VBR.contains(&q).then_some(ExportQuality::Vbr(q))
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
            file: ExportFile::Aiff,
            dry: true,
            locators: true,
            selection: false,
            ..ExportValues::default()
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
    fn flac_is_remembered_and_never_with_float() {
        let flac = ExportValues {
            file: ExportFile::Flac,
            depth: ExportDepth::Pcm16,
            ..ExportValues::default()
        };
        let prefs = ExportPrefs::from_values(&flac, None);
        assert_eq!(prefs.file, "flac");
        assert_eq!(prefs.values(), flac);
        let both = ExportPrefs {
            file: "flac".to_owned(),
            bit_depth: "32f".to_owned(),
            ..ExportPrefs::default()
        };
        assert_eq!(both.values().depth, ExportDepth::Pcm24);
        assert_eq!(both.values().file, ExportFile::Flac);
    }

    #[test]
    fn the_last_compressed_format_and_its_quality_are_remembered() {
        let d = dir("lossy");
        let path = d.join(EXPORT_FILE);
        for (file, mp3, aac, opus) in [
            (ExportFile::Mp3, ExportQuality::Vbr(2), 256, 96),
            (ExportFile::Mp3, ExportQuality::Cbr(320), 128, 192),
            (ExportFile::Aac, ExportQuality::Cbr(256), 320, 64),
            (ExportFile::Opus, ExportQuality::Cbr(128), 192, 160),
        ] {
            let values = ExportValues {
                file,
                mp3,
                aac,
                opus,
                ..ExportValues::default()
            };
            ExportPrefs::from_values(&values, None).save(&path).unwrap();
            assert_eq!(ExportPrefs::load(&path).values(), values);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unknown_quality_is_the_default_and_an_old_file_has_the_default_quality() {
        let d = dir("lossy-lenient");
        let path = d.join(EXPORT_FILE);
        let default = ExportValues::default();
        std::fs::write(
            &path,
            r#"{"file":"mp3","mp3":"cbr999","aac_kbps":7,"opus_kbps":12345}"#,
        )
        .unwrap();
        let v = ExportPrefs::load(&path).values();
        assert_eq!(v.file, ExportFile::Mp3);
        assert_eq!(
            (v.mp3, v.aac, v.opus),
            (default.mp3, default.aac, default.opus)
        );
        std::fs::write(&path, r#"{"file":"opus","mp3":"v9"}"#).unwrap();
        assert_eq!(ExportPrefs::load(&path).values().mp3, default.mp3);
        // A file written before the compressed formats existed keeps what it says.
        std::fs::write(&path, r#"{"file":"flac","bit_depth":"16"}"#).unwrap();
        let v = ExportPrefs::load(&path).values();
        assert_eq!((v.file, v.depth), (ExportFile::Flac, ExportDepth::Pcm16));
        assert_eq!(v.mp3, default.mp3);
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
