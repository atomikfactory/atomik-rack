//! File rows and their classification.

use crate::item::{CLIP_EXT, CONTAINER_EXT, ItemInfo, PATCH_EXT, PROJECT_EXT};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// An audio container the app can decode and preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AudioFormat {
    /// `.wav`, `.wave`
    Wav,
    /// `.aif`, `.aiff`, `.aifc`
    Aiff,
    /// `.flac`
    Flac,
    /// `.mp3`
    Mp3,
    /// `.ogg`, `.oga`
    Ogg,
}

/// What a browser row is. Files that are neither folders, audio nor library items are not rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FileKind {
    /// A directory.
    Folder,
    /// An audio file of the given format.
    Audio(AudioFormat),
    /// A patch item (`.atomikpatch`).
    Patch,
    /// A container item (`.atomikcontainer`).
    Container,
    /// A note clip item (`.atomikclip`).
    Clip,
    /// A project file (`.atomik`).
    Project,
    /// A built-in device: a virtual row whose path is a device URI, never a file.
    Device,
}

impl FileKind {
    /// Whether the row is a library item (anything but a folder or an audio file).
    pub fn is_item(self) -> bool {
        !matches!(self, Self::Folder | Self::Audio(_))
    }
}

/// Classifies a directory entry: folders are always rows, files only when their extension
/// (case-insensitive) is a supported audio format or a library item. Returns `None` for
/// everything else. Never returns [`FileKind::Device`].
pub fn classify(path: &Path, is_dir: bool) -> Option<FileKind> {
    if is_dir {
        return Some(FileKind::Folder);
    }
    let ext = path.extension()?.to_str()?;
    let is = |candidates: &[&str]| candidates.iter().any(|c| ext.eq_ignore_ascii_case(c));
    for (candidate, kind) in [
        (PATCH_EXT, FileKind::Patch),
        (CONTAINER_EXT, FileKind::Container),
        (CLIP_EXT, FileKind::Clip),
        (PROJECT_EXT, FileKind::Project),
    ] {
        if is(&[candidate]) {
            return Some(kind);
        }
    }
    let format = if is(&["wav", "wave"]) {
        AudioFormat::Wav
    } else if is(&["aif", "aiff", "aifc"]) {
        AudioFormat::Aiff
    } else if is(&["flac"]) {
        AudioFormat::Flac
    } else if is(&["mp3"]) {
        AudioFormat::Mp3
    } else if is(&["ogg", "oga"]) {
        AudioFormat::Ogg
    } else {
        return None;
    };
    Some(FileKind::Audio(format))
}

/// Stream parameters of an audio file, read from its header.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioInfo {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channel count (at least 1).
    pub channels: u16,
    /// Total frames, when the header states it (some MP3/OGG files do not).
    pub frames: Option<u64>,
}

impl AudioInfo {
    /// `frames / sample_rate`, or `None` when the length is unknown or the rate is 0.
    pub fn duration_secs(&self) -> Option<f64> {
        if self.sample_rate == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)] // frame counts stay far below 2^53
        let frames = self.frames? as f64;
        Some(frames / f64::from(self.sample_rate))
    }
}

/// One entry of a browser listing (a folder, an audio file or a library item).
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    /// Absolute path.
    pub path: PathBuf,
    /// Display name (the file name with its extension).
    pub name: String,
    /// Folder or audio format.
    pub kind: FileKind,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Header data, filled in later by the probe worker.
    pub info: Option<AudioInfo>,
    /// Detail and category of a library item, filled in later by the probe worker (devices
    /// carry theirs from the start).
    pub item: Option<ItemInfo>,
    /// Favourite flag from the library database.
    pub favorite: bool,
    /// Tags from the library database, in display order.
    pub tags: Vec<String>,
    /// Author from the library database (empty when none was given).
    pub author: String,
    /// Categories the user gave the file, from the library database, in display order.
    pub categories: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(name: &str) -> Option<FileKind> {
        classify(Path::new(name), false)
    }

    #[test]
    fn folders_are_always_rows() {
        assert_eq!(classify(Path::new("x.txt"), true), Some(FileKind::Folder));
        assert_eq!(classify(Path::new("noext"), true), Some(FileKind::Folder));
    }

    #[test]
    fn recognises_every_extension() {
        use AudioFormat::*;
        let cases = [
            ("a.wav", Wav),
            ("a.wave", Wav),
            ("a.aif", Aiff),
            ("a.aiff", Aiff),
            ("a.aifc", Aiff),
            ("a.flac", Flac),
            ("a.mp3", Mp3),
            ("a.ogg", Ogg),
            ("a.oga", Ogg),
        ];
        for (name, fmt) in cases {
            assert_eq!(audio(name), Some(FileKind::Audio(fmt)), "{name}");
        }
    }

    #[test]
    fn extension_is_case_insensitive() {
        assert_eq!(audio("KICK.WAV"), Some(FileKind::Audio(AudioFormat::Wav)));
        assert_eq!(audio("x.FlAc"), Some(FileKind::Audio(AudioFormat::Flac)));
    }

    #[test]
    fn other_files_are_not_rows() {
        assert_eq!(audio("readme.txt"), None);
        assert_eq!(audio("song.mp4"), None);
        assert_eq!(audio("noext"), None);
        assert_eq!(audio("wav"), None);
        assert_eq!(audio("kick.wav.bak"), None);
        assert_eq!(audio("kick.wav "), None);
    }

    #[test]
    fn bare_dotfile_has_no_extension() {
        // std treats ".wav" as a stem without extension, so it is not audio.
        assert_eq!(audio(".wav"), None);
        assert_eq!(
            audio(".hidden.wav"),
            Some(FileKind::Audio(AudioFormat::Wav))
        );
    }

    #[test]
    fn uses_only_the_last_component() {
        assert_eq!(audio("some.wav/readme"), None);
    }

    fn info(rate: u32, frames: Option<u64>) -> AudioInfo {
        AudioInfo {
            sample_rate: rate,
            channels: 2,
            frames,
        }
    }

    #[test]
    fn duration_is_frames_over_rate() {
        assert_eq!(info(48_000, Some(96_000)).duration_secs(), Some(2.0));
        assert_eq!(info(44_100, Some(0)).duration_secs(), Some(0.0));
    }

    #[test]
    fn duration_unknown_without_frames_or_rate() {
        assert_eq!(info(48_000, None).duration_secs(), None);
        assert_eq!(info(0, Some(10)).duration_secs(), None);
    }
}
