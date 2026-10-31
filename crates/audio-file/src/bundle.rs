//! The project folder bundle: `Audio/<sha256>.wav` (32-bit float) plus cached
//! waveform peaks in `Audio/.peaks/<sha256>.peaks`. Assets are immutable and named by the
//! content hash of their decoded samples, so importing the same audio twice stores it once.
//!
//! Every file is written to a temporary file in the same directory (`<name>.tmp~`), flushed,
//! fsynced and renamed into place, so a crash never leaves a partial file under a real name.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::data::AudioData;
use crate::decode::decode_file_with;
use crate::error::{AudioFileError, Result};
use crate::hash::content_hash;
use crate::peaks::PeakPyramid;
use crate::resample::resample;
use crate::wav::{WavStreamReader, read_wav_f32, write_wav_f32};

const AUDIO_DIR: &str = "Audio";
const PEAKS_DIR: &str = ".peaks";
const TEMP_SUFFIX: &str = ".tmp~";
/// File name returned by [`Bundle::asset_path`] for a malformed hash (never exists).
const INVALID_NAME: &str = "_invalid-hash.wav";

/// What the model stores about an asset in the sample pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetInfo {
    /// Lowercase SHA-256 hex of the stored (converted) audio; the asset id.
    pub hash: String,
    /// Channel count.
    pub channels: u16,
    /// Sample rate of the stored audio in Hz.
    pub sample_rate: u32,
    /// Length in frames.
    pub frames: u64,
    /// Display name: the imported file's name, or the name given to a recording.
    pub original_name: String,
    /// Samples of the imported file that were NaN or infinite and were stored as silence (zero
    /// for a sound file and for a recording).
    pub replaced_samples: u64,
}

/// True for 64 lowercase hex digits, the only form accepted as an asset id.
pub fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_hash(hash: &str) -> Result<()> {
    if is_valid_hash(hash) {
        Ok(())
    } else {
        Err(AudioFileError::InvalidData(format!(
            "malformed asset hash {hash:?}"
        )))
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(TEMP_SUFFIX);
    path.with_file_name(name)
}

/// Runs `write` against a temporary sibling of `path`, fsyncs it and renames it over `path`.
/// On any failure the temporary file is removed and `path` is left as it was.
pub(crate) fn write_atomic(path: &Path, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let tmp = temp_path(path);
    let result = (|| {
        write(&tmp)?;
        OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
        fs::rename(&tmp, path)?;
        sync_parent_dir(path);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Makes the rename durable on Unix (best effort; directories cannot be opened on Windows).
#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Ok(f) = fs::File::open(dir)
    {
        let _ = f.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

/// Writes `contents` to `path` atomically: temp file in the same directory, flush, fsync, then
/// rename over the target. The old file survives any failure; no partial file is ever visible.
pub fn save_text_atomic(path: &Path, contents: &str) -> Result<()> {
    write_atomic(path, |tmp| {
        fs::write(tmp, contents.as_bytes())?;
        Ok(())
    })
}

/// A project folder with its `Audio/` store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    root: PathBuf,
}

impl Bundle {
    /// Creates `root`, `root/Audio` and `root/Audio/.peaks` (all missing parents too). Harmless
    /// when they already exist.
    pub fn create(root: &Path) -> Result<Self> {
        let b = Self {
            root: root.to_path_buf(),
        };
        fs::create_dir_all(b.peaks_dir())?;
        Ok(b)
    }

    /// Opens an existing project folder. `Audio/` is created on first write, so a project
    /// without audio keeps working.
    pub fn open(root: &Path) -> Result<Self> {
        if !root.is_dir() {
            return Err(AudioFileError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("project folder not found: {}", root.display()),
            )));
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// The project folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `root/Audio`.
    pub fn audio_dir(&self) -> PathBuf {
        self.root.join(AUDIO_DIR)
    }

    /// `root/Audio/.peaks`.
    pub fn peaks_dir(&self) -> PathBuf {
        self.audio_dir().join(PEAKS_DIR)
    }

    /// `root/Audio/<hash>.wav`. A malformed hash maps to a fixed non-existent name inside
    /// `Audio/`, so a hostile project file cannot point outside the bundle.
    pub fn asset_path(&self, hash: &str) -> PathBuf {
        if is_valid_hash(hash) {
            self.audio_dir().join(format!("{hash}.wav"))
        } else {
            self.audio_dir().join(INVALID_NAME)
        }
    }

    /// `root/Audio/.peaks/<hash>.peaks` (same rule for malformed hashes).
    pub fn peaks_path(&self, hash: &str) -> PathBuf {
        if is_valid_hash(hash) {
            self.peaks_dir().join(format!("{hash}.peaks"))
        } else {
            self.peaks_dir().join("_invalid-hash.peaks")
        }
    }

    /// Whether the asset file is present.
    pub fn exists(&self, hash: &str) -> bool {
        is_valid_hash(hash) && self.asset_path(hash).is_file()
    }

    /// Decodes `src`, converts it to `session_rate` if its rate differs, and stores it
    /// under its content hash unless that asset is already present. Importing the same audio
    /// again changes nothing on disk and returns an equal [`AssetInfo`].
    ///
    /// A file of more than [`crate::DEFAULT_MAX_SAMPLES`] samples is refused, and NaN or
    /// infinite samples are stored as silence (counted in [`AssetInfo::replaced_samples`]).
    pub fn import(&mut self, src: &Path, session_rate: u32) -> Result<AssetInfo> {
        let decoded = decode_file_with(src, crate::DEFAULT_MAX_SAMPLES)?;
        let data = resample(&decoded.audio, session_rate)?;
        let name = src.file_name().map_or_else(
            || src.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut info = self.store(&data, &name)?;
        info.replaced_samples = decoded.replaced;
        Ok(info)
    }

    /// Stores already-decoded audio (a finished recording) the same way, without decoding.
    pub fn add_recording(&mut self, data: &AudioData, name: &str) -> Result<AssetInfo> {
        self.store(data, name)
    }

    /// Resamples the stored asset `hash` to `rate` and stores the result as a new asset named
    /// `name` (the original stays). The copy has `round(frames * rate / old rate)` frames and
    /// is time-aligned with the original; a stored asset already at `rate` returns its own
    /// info.
    pub fn convert_rate(&mut self, hash: &str, rate: u32, name: &str) -> Result<AssetInfo> {
        let data = self.load(hash)?;
        let converted = resample(&data, rate)?;
        self.store(&converted, name)
    }

    fn store(&mut self, data: &AudioData, name: &str) -> Result<AssetInfo> {
        data.validate()?;
        if data.frames() == 0 {
            return Err(AudioFileError::Empty);
        }
        let hash = content_hash(data);
        fs::create_dir_all(self.peaks_dir())?;
        let asset = self.asset_path(&hash);
        if !asset.is_file() {
            write_atomic(&asset, |tmp| write_wav_f32(tmp, data))?;
        }
        let peaks = self.peaks_path(&hash);
        if !peaks.is_file() {
            self.write_peaks(&hash, &PeakPyramid::build(data)?)?;
        }
        Ok(AssetInfo {
            hash,
            channels: data.channels.len() as u16,
            sample_rate: data.sample_rate,
            frames: data.frames() as u64,
            original_name: name.to_owned(),
            replaced_samples: 0,
        })
    }

    fn write_peaks(&self, hash: &str, pyramid: &PeakPyramid) -> Result<()> {
        let bytes = pyramid.to_bytes();
        write_atomic(&self.peaks_path(hash), |tmp| {
            fs::write(tmp, &bytes)?;
            Ok(())
        })
    }

    fn require(&self, hash: &str) -> Result<PathBuf> {
        check_hash(hash)?;
        let path = self.asset_path(hash);
        if path.is_file() {
            Ok(path)
        } else {
            Err(AudioFileError::AssetMissing {
                hash: hash.to_owned(),
            })
        }
    }

    /// Loads a whole asset into memory. A NaN or infinite sample reads as silence.
    pub fn load(&self, hash: &str) -> Result<AudioData> {
        read_wav_f32(&self.require(hash)?)
    }

    /// [`Self::load`] that also counts the non-finite samples it replaced (an asset this crate
    /// stored has none; a file placed in the bundle by hand can).
    pub fn load_counted(&self, hash: &str) -> Result<crate::Decoded> {
        crate::read_wav_f32_limited(&self.require(hash)?, u64::MAX)
    }

    /// Opens a seekable streaming reader over an asset (for assets too large for RAM).
    pub fn stream_reader(&self, hash: &str) -> Result<WavStreamReader> {
        WavStreamReader::open(&self.require(hash)?)
    }

    /// Loads the cached peak pyramid. A missing or unreadable cache is regenerated from the
    /// asset (and written back, best effort); a missing asset is `AssetMissing`.
    pub fn load_peaks(&self, hash: &str) -> Result<PeakPyramid> {
        let asset = self.require(hash)?;
        if let Ok(bytes) = fs::read(self.peaks_path(hash))
            && let Ok(pyramid) = PeakPyramid::from_bytes(&bytes)
        {
            return Ok(pyramid);
        }
        let pyramid = PeakPyramid::build(&read_wav_f32(&asset)?)?;
        let _ = fs::create_dir_all(self.peaks_dir());
        let _ = self.write_peaks(hash, &pyramid);
        Ok(pyramid)
    }

    /// Copies the `Audio/` tree (assets and peak caches) of `from_root` into `to_root`, for
    /// Save As. Files already present with the same size are kept (assets are immutable). The
    /// project file itself is the caller's to write with [`save_text_atomic`].
    pub fn copy_bundle(from_root: &Path, to_root: &Path) -> Result<Bundle> {
        let from = Bundle::open(from_root)?;
        let to = Bundle::create(to_root)?;
        copy_dir(&from.audio_dir(), &to.audio_dir())?;
        Ok(to)
    }

    /// Asset hashes present in `Audio/` that are not in `referenced`, sorted. An unreadable
    /// `Audio/` directory reports no orphans.
    pub fn orphans(&self, referenced: &BTreeSet<String>) -> Vec<String> {
        let mut found = BTreeSet::new();
        if let Ok(entries) = fs::read_dir(self.audio_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() || path.extension().is_none_or(|e| e != "wav") {
                    continue;
                }
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    && is_valid_hash(stem)
                    && !referenced.contains(stem)
                {
                    found.insert(stem.to_owned());
                }
            }
        }
        found.into_iter().collect()
    }

    /// Deletes the orphans (and their peak caches) plus stale `*.tmp~` leftovers from crashed
    /// writes. Returns the removed hashes, sorted. Other files in `Audio/` are never touched.
    pub fn remove_orphans(&mut self, referenced: &BTreeSet<String>) -> Result<Vec<String>> {
        let orphans = self.orphans(referenced);
        for hash in &orphans {
            remove_if_present(&self.asset_path(hash))?;
            remove_if_present(&self.peaks_path(hash))?;
        }
        for dir in [self.audio_dir(), self.peaks_dir()] {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.to_string_lossy().ends_with(TEMP_SUFFIX) {
                        remove_if_present(&path)?;
                    }
                }
            }
        }
        Ok(orphans)
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    let entries = match fs::read_dir(from) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut paths: Vec<PathBuf> = entries
        .collect::<io::Result<Vec<_>>>()?
        .iter()
        .map(|e| e.path())
        .collect();
    paths.sort();
    for src in paths {
        let Some(name) = src.file_name() else {
            continue;
        };
        let dst = to.join(name);
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else if !src.to_string_lossy().ends_with(TEMP_SUFFIX) {
            let same = fs::metadata(&dst)
                .is_ok_and(|d| d.len() == fs::metadata(&src).map_or(u64::MAX, |s| s.len()));
            if !same {
                write_atomic(&dst, |tmp| {
                    fs::copy(&src, tmp)?;
                    Ok(())
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join("atomik-audio-file-bundle-unit")
            .join(format!("{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn failure_midway_through_the_write_leaves_the_target_untouched_and_no_temp() {
        let d = dir("midway");
        let target = d.join("project.atomik");
        save_text_atomic(&target, "old contents").unwrap();
        let err = write_atomic(&target, |tmp| {
            fs::write(tmp, b"new conte")?; // partial write, then the "disk" fails
            Err(AudioFileError::Io(io::Error::other("simulated disk full")))
        });
        assert!(err.is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "old contents");
        assert!(!temp_path(&target).exists());
        let names: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["project.atomik"]);
    }

    #[test]
    fn failure_before_the_first_save_creates_no_target() {
        let d = dir("first");
        let target = d.join("new.atomik");
        let err = write_atomic(&target, |tmp| {
            fs::write(tmp, b"partial")?;
            Err(AudioFileError::Io(io::Error::other("simulated crash")))
        });
        assert!(err.is_err());
        assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
    }

    #[test]
    fn hash_validation() {
        assert!(is_valid_hash(&"ab".repeat(32)));
        assert!(!is_valid_hash(&"AB".repeat(32)));
        assert!(!is_valid_hash(&"ab".repeat(31)));
    }
}
