//! Export presets: one choice for a purpose, without knowing the formats.
//!
//! A preset is a name and what the export panel's fields say: the container, depth, rate,
//! channels, codecs, stems or the mix, the range and the loudness target
//! ([`atomik_ui::PresetChoice`]). Choosing one in the panel's Preset row fills the fields;
//! **File > Export for** runs one directly.
//!
//! The built-in presets are in code ([`builtin`]) and cannot be deleted. Their names say the
//! purpose and no service:
//!
//! * **Streaming**: a 24-bit WAV at the project's rate and an MP3 at 320 kbps, both brought to
//!   -14 LUFS with the true peak under -1 dBTP.
//! * **Podcast**: a mono MP3 at 128 kbps at -16 LUFS.
//! * **Video**: AAC at 256 kbps and 48 kHz at -14 LUFS; with a film in the song, the film is
//!   exported with that mix (File > Export > Film with Mix).
//! * **Stems for mixing**: one 24-bit WAV per strip, no loudness change, no dither, from the
//!   start of the song to its end.
//! * **Club**: a 24-bit WAV with -9 LUFS as a guide only: no gain and no limiter are added, and
//!   the report warns when the peaks clip.
//!
//! A preset that writes a compressed file needs FFmpeg ([`PresetChoice::needs_ffmpeg`]).
//!
//! The user's own presets are made from the panel's current fields (Save Preset) and live in
//! `export_presets.json` next to `settings.json`:
//!
//! ```json
//! { "version": 1,
//!   "presets": [ { "name": "My master", "stems": false,
//!                  "values": { "file": "flac", "bit_depth": "24", "loudness": "guide-9" } } ] }
//! ```
//!
//! The values use the words of `export.json` ([`crate::export_prefs`]). Loading is lenient: a
//! missing or unreadable file, or one of another version, is no presets; an entry with a blank
//! or repeated name, or the name of a built-in preset, is skipped, and a field this build does
//! not know is its default. Saving is atomic and canonical.

use crate::export_prefs::ExportPrefs;
use atomik_ui::{
    ExportDither, ExportFile, ExportLoudness, ExportQuality, ExportValues, PresetChoice,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name.
pub const PRESETS_FILE: &str = "export_presets.json";

/// The version written to the file.
pub const VERSION: u32 = 1;

/// The most presets of the user's.
pub const MAX_USER_PRESETS: usize = 16;

/// The longest name of a preset, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// The name of the streaming preset.
pub const STREAMING: &str = "Streaming";
/// The name of the podcast preset.
pub const PODCAST: &str = "Podcast";
/// The name of the video preset.
pub const VIDEO: &str = "Video";
/// The name of the stems preset.
pub const STEMS: &str = "Stems for mixing";
/// The name of the club preset.
pub const CLUB: &str = "Club";

/// The presets file next to a settings file.
pub fn presets_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(PRESETS_FILE)
}

/// The built-in presets, in the order the lists show them.
pub fn builtin() -> Vec<PresetChoice> {
    let preset = |name: &str, stems: bool, values: ExportValues| PresetChoice {
        name: name.to_owned(),
        stems,
        values,
        user: false,
    };
    vec![
        preset(
            STREAMING,
            false,
            ExportValues {
                loudness: ExportLoudness::Target(-14),
                also_mp3: true,
                ..ExportValues::default()
            },
        ),
        preset(
            PODCAST,
            false,
            ExportValues {
                file: ExportFile::Mp3,
                mp3: ExportQuality::Cbr(128),
                mono: true,
                loudness: ExportLoudness::Target(-16),
                ..ExportValues::default()
            },
        ),
        preset(
            VIDEO,
            false,
            ExportValues {
                file: ExportFile::Aac,
                aac: 256,
                rate: Some(48_000),
                loudness: ExportLoudness::Target(-14),
                ..ExportValues::default()
            },
        ),
        preset(
            STEMS,
            true,
            ExportValues {
                dither: ExportDither::Off,
                ..ExportValues::default()
            },
        ),
        preset(
            CLUB,
            false,
            ExportValues {
                loudness: ExportLoudness::Guide(-9),
                ..ExportValues::default()
            },
        ),
    ]
}

/// Whether `name` is one of the built-in presets (compared without regard to case).
pub fn is_builtin(name: &str) -> bool {
    builtin().iter().any(|p| p.name.eq_ignore_ascii_case(name))
}

/// What a preset stores: the words of `export.json` for the fields a preset sets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    file: String,
    bit_depth: String,
    dither: String,
    sample_rate: Option<u32>,
    tail: bool,
    mp3: String,
    aac_kbps: u16,
    opus_kbps: u16,
    dry: bool,
    locators: bool,
    mono: bool,
    loudness: String,
    also_mp3: bool,
}

impl Default for Stored {
    fn default() -> Self {
        Self::of(&ExportValues::default())
    }
}

impl Stored {
    fn of(values: &ExportValues) -> Self {
        let p = ExportPrefs::from_values(values, None);
        Self {
            file: p.file,
            bit_depth: p.bit_depth,
            dither: p.dither,
            sample_rate: p.sample_rate,
            tail: p.tail,
            mp3: p.mp3,
            aac_kbps: p.aac_kbps,
            opus_kbps: p.opus_kbps,
            dry: p.dry,
            locators: p.locators,
            mono: p.mono,
            loudness: p.loudness,
            also_mp3: p.also_mp3,
        }
    }

    /// The values: what this build does not know is the default.
    fn values(&self) -> ExportValues {
        ExportPrefs {
            file: self.file.clone(),
            bit_depth: self.bit_depth.clone(),
            dither: self.dither.clone(),
            sample_rate: self.sample_rate,
            tail: self.tail,
            mp3: self.mp3.clone(),
            aac_kbps: self.aac_kbps,
            opus_kbps: self.opus_kbps,
            dry: self.dry,
            locators: self.locators,
            mono: self.mono,
            loudness: self.loudness.clone(),
            also_mp3: self.also_mp3,
            ..ExportPrefs::default()
        }
        .values()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredPreset {
    name: String,
    #[serde(default)]
    stems: bool,
    #[serde(default)]
    values: Stored,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredFile {
    version: u32,
    #[serde(default)]
    presets: Vec<StoredPreset>,
}

/// The user's presets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresetStore {
    user: Vec<PresetChoice>,
}

/// Why a name cannot be used: `None` when it can.
fn name_refusal(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Type a name for the preset".to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Some(format!(
            "The name is too long: {MAX_NAME_CHARS} characters at most"
        ));
    }
    if name.chars().any(char::is_control) {
        return Some("The name cannot hold control characters".to_owned());
    }
    if is_builtin(name) {
        return Some(format!("{name} is a built-in preset: pick another name"));
    }
    None
}

impl PresetStore {
    /// The user's presets.
    pub fn user(&self) -> &[PresetChoice] {
        &self.user
    }

    /// Every preset: the built-in ones, then the user's.
    pub fn all(&self) -> Vec<PresetChoice> {
        let mut all = builtin();
        all.extend(self.user.iter().cloned());
        all
    }

    /// The preset at `index` of [`Self::all`].
    pub fn get(&self, index: usize) -> Option<PresetChoice> {
        self.all().into_iter().nth(index)
    }

    /// Keeps the fields as the user's preset `name` (trimmed): a preset of that name (any case)
    /// is replaced where it stands, else the preset goes last. Returns the name as kept.
    ///
    /// # Errors
    /// The name is blank, too long, has control characters or is a built-in preset's, or there
    /// are [`MAX_USER_PRESETS`] already.
    pub fn add(
        &mut self,
        name: &str,
        stems: bool,
        mut values: ExportValues,
    ) -> Result<String, String> {
        let name = name.trim().to_owned();
        if let Some(why) = name_refusal(&name) {
            return Err(why);
        }
        if stems {
            // A stems export has no mix-down, loudness or extra file: they are not kept.
            values.mono = false;
            values.loudness = ExportLoudness::Off;
            values.also_mp3 = false;
        }
        if values.file.is_lossy() {
            values.also_mp3 = false;
        }
        // The time selection belongs to the song that is open.
        values.selection = false;
        let preset = PresetChoice {
            name: name.clone(),
            stems,
            values,
            user: true,
        };
        match self
            .user
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(&name))
        {
            Some(i) => self.user[i] = preset,
            None if self.user.len() >= MAX_USER_PRESETS => {
                return Err(format!(
                    "There are {MAX_USER_PRESETS} presets of yours: delete one first"
                ));
            }
            None => self.user.push(preset),
        }
        Ok(name)
    }

    /// Deletes the user's preset `name`; false when there is none (a built-in preset is never
    /// deleted).
    pub fn delete(&mut self, name: &str) -> bool {
        let before = self.user.len();
        self.user.retain(|p| p.name != name);
        self.user.len() != before
    }

    /// Pretty JSON for the file.
    pub fn to_json(&self) -> String {
        let file = StoredFile {
            version: VERSION,
            presets: self
                .user
                .iter()
                .map(|p| StoredPreset {
                    name: p.name.clone(),
                    stems: p.stems,
                    values: Stored::of(&p.values),
                })
                .collect(),
        };
        // Plain data with string keys: serialization cannot fail.
        let mut json = serde_json::to_string_pretty(&file).expect("export presets serialize");
        json.push('\n');
        json
    }

    /// Reads the presets from `text`: none when it is not a presets file of this version.
    pub fn from_json(text: &str) -> Self {
        let Ok(file) = serde_json::from_str::<StoredFile>(text) else {
            return Self::default();
        };
        if file.version != VERSION {
            return Self::default();
        }
        let mut store = Self::default();
        for entry in file.presets {
            let name = entry.name.trim().to_owned();
            if name_refusal(&name).is_some()
                || store
                    .user
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(&name))
                || store.user.len() >= MAX_USER_PRESETS
            {
                continue;
            }
            store.user.push(PresetChoice {
                name,
                stems: entry.stems,
                values: entry.values.values(),
                user: true,
            });
        }
        store
    }

    /// Reads the presets at `path`: none when the file is missing or unusable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::from_json(&text))
            .unwrap_or_default()
    }

    /// Writes the presets to `path`.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::keymap_store::write_atomic(path, &self.to_json())
    }
}

#[cfg(test)]
#[path = "export_presets_tests.rs"]
mod tests;
