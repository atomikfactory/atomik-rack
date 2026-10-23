//! Finding the files of missing audio assets in a folder.
//!
//! [`scan_folder`] walks a folder breadth first (bounded: [`MAX_DEPTH`] levels, [`MAX_ENTRIES`]
//! directory entries, symbolic links not followed) and matches the wanted assets by their
//! original file name, case-insensitively. The walk is deterministic: the entries of a folder
//! are visited in file-name order and shallower files come first. When several files share a
//! wanted name, the one whose header (channels and length, at the asset's rate) agrees with the
//! asset is preferred, else the first found. Reading a header is cheap (no decoding), so a
//! content hash is never computed here; the import that follows decides whether the match is the
//! very same audio (same hash: the file is simply restored) or a replacement.

use crate::audio_import::is_audio_path;
use atomik_audio_file::probe_file;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

/// Deepest sub-folder level a scan enters (the folder itself is level 0).
pub const MAX_DEPTH: usize = 8;
/// Directory entries a scan looks at before it stops.
pub const MAX_ENTRIES: usize = 50_000;

/// A missing asset a scan looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// Asset id.
    pub hash: String,
    /// The original file name to match (any case).
    pub name: String,
    /// Channel count of the stored asset.
    pub channels: u16,
    /// Sample rate of the stored asset.
    pub sample_rate: u32,
    /// Length of the stored asset in frames.
    pub frames: u64,
}

/// What a scan found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanResult {
    /// `(asset id, file)` for each wanted asset a file was found for, in the order of the
    /// wanted list.
    pub found: Vec<(String, PathBuf)>,
    /// Directory entries looked at.
    pub entries: usize,
    /// The scan stopped at [`MAX_ENTRIES`].
    pub truncated: bool,
}

/// Looks for the files of `wanted` under `root` (see the module docs).
pub fn scan_folder(root: &Path, wanted: &[Wanted]) -> ScanResult {
    let lowered: Vec<String> = wanted.iter().map(|w| w.name.to_lowercase()).collect();
    let mut candidates: Vec<Vec<PathBuf>> = vec![Vec::new(); wanted.len()];
    let mut result = ScanResult::default();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::from([(root.to_path_buf(), 0)]);
    'walk: while let Some((dir, depth)) = queue.pop_front() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
        for entry in read.flatten() {
            result.entries += 1;
            if result.entries > MAX_ENTRIES {
                result.truncated = true;
                break 'walk;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.push((name, entry.path(), kind.is_dir()));
        }
        entries.sort_by(|a, b| {
            a.0.to_lowercase()
                .cmp(&b.0.to_lowercase())
                .then(a.0.cmp(&b.0))
        });
        for (name, path, is_dir) in entries {
            if is_dir {
                if depth < MAX_DEPTH {
                    queue.push_back((path, depth + 1));
                }
            } else if is_audio_path(&path) {
                let name = name.to_lowercase();
                for (i, want) in lowered.iter().enumerate() {
                    if *want == name {
                        candidates[i].push(path.clone());
                    }
                }
            }
        }
    }
    for (want, paths) in wanted.iter().zip(candidates) {
        if let Some(path) = pick(want, paths) {
            result.found.push((want.hash.clone(), path));
        }
    }
    result
}

/// The candidate whose header agrees with the asset, else the first.
fn pick(want: &Wanted, paths: Vec<PathBuf>) -> Option<PathBuf> {
    if paths.len() > 1
        && let Some(agreeing) = paths.iter().find(|p| header_agrees(want, p))
    {
        return Some(agreeing.clone());
    }
    paths.into_iter().next()
}

fn header_agrees(want: &Wanted, path: &Path) -> bool {
    let Ok(Ok(info)) = catch_unwind(AssertUnwindSafe(|| probe_file(path))) else {
        return false;
    };
    let Some(frames) = info.frames else {
        return false;
    };
    if info.channels != want.channels || info.sample_rate == 0 {
        return false;
    }
    // The asset was converted to its own rate on import: compare lengths at that rate.
    let converted =
        u128::from(frames) * u128::from(want.sample_rate) / u128::from(info.sample_rate);
    converted.abs_diff(u128::from(want.frames)) <= 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_audio_file::{AudioData, write_wav_f32};

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "atomik-app-relink-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn want(hash: &str, name: &str, frames: u64) -> Wanted {
        Wanted {
            hash: hash.to_owned(),
            name: name.to_owned(),
            channels: 1,
            sample_rate: 48_000,
            frames,
        }
    }

    fn write(path: &Path, frames: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_wav_f32(
            path,
            &AudioData::new(48_000, vec![vec![0.1; frames]]).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn names_match_in_any_case_in_nested_folders_and_others_stay_unmatched() {
        let t = Tmp::new("names");
        write(&t.0.join("a").join("b").join("KICK.WAV"), 100);
        write(&t.0.join("other.wav"), 100);
        std::fs::write(t.0.join("kick.txt"), b"not audio").unwrap();
        let wanted = [want("h1", "kick.wav", 100), want("h2", "snare.wav", 100)];
        let r = scan_folder(&t.0, &wanted);
        assert_eq!(
            r.found,
            [("h1".to_owned(), t.0.join("a").join("b").join("KICK.WAV"))]
        );
        assert!(!r.truncated);
    }

    #[test]
    fn the_shallowest_file_wins_and_a_header_that_agrees_beats_it() {
        let t = Tmp::new("ties");
        write(&t.0.join("kick.wav"), 50);
        write(&t.0.join("deep").join("kick.wav"), 100);
        let r = scan_folder(&t.0, &[want("h", "kick.wav", 100)]);
        assert_eq!(
            r.found[0].1,
            t.0.join("deep").join("kick.wav"),
            "the length agrees with the second file only"
        );
        let r = scan_folder(&t.0, &[want("h", "kick.wav", 7)]);
        assert_eq!(
            r.found[0].1,
            t.0.join("kick.wav"),
            "no agreement: the first"
        );
    }

    #[test]
    fn the_walk_is_bounded_in_depth_and_in_entries() {
        let t = Tmp::new("bounds");
        let mut deep = t.0.clone();
        for level in 0..=MAX_DEPTH + 1 {
            deep = deep.join(format!("d{level}"));
        }
        write(&deep.join("far.wav"), 10);
        let near = t.0.join("d0").join("d1");
        write(&near.join("near.wav"), 10);
        let r = scan_folder(
            &t.0,
            &[want("far", "far.wav", 10), want("near", "near.wav", 10)],
        );
        assert_eq!(r.found.len(), 1, "{r:?}");
        assert_eq!(r.found[0].0, "near");
    }

    #[test]
    fn a_missing_folder_finds_nothing() {
        let r = scan_folder(
            Path::new("/no/such/folder/anywhere"),
            &[want("h", "a.wav", 1)],
        );
        assert_eq!(r, ScanResult::default());
    }
}
