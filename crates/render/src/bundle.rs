//! Project bundles for the app and the bounce.
//!
//! # Layout rules
//!
//! A project with audio is a **folder bundle**: `<name>/` holding `<name>.atomik` (the project
//! JSON) and `Audio/` (assets named by content hash, plus the `Audio/.peaks/` cache), see
//! [`atomik_audio_file::Bundle`]. A project without audio stays a single `.atomik` file.
//!
//! * [`existing_bundle_root`] finds the folder that holds (or would hold) the audio of an
//!   opened project file, in this order: the file's own folder when that folder is named
//!   like the file (`song/song.atomik`); a sibling folder named like the file
//!   (`song.atomik` next to `song/`, the layout of the repository examples); the file's folder
//!   when it already has an `Audio/` directory. No match: the project has no bundle.
//! * [`save_as_layout`] decides where "Save As" puts a project with audio: when the picked
//!   file already sits in a folder named like it, the file stays; otherwise a new folder
//!   `<name>/` is made next to the picked path and the file goes inside it.
//! * [`resolve_project_path`] accepts either a `.atomik` file or a bundle folder.
//!
//! [`load_song_assets`] reads every referenced asset fully into memory for a bounce and fails
//! with a message that names each missing asset.

use atomik_audio_file::Bundle;
use atomik_engine::SongAsset;
use atomik_model::RackDoc;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The extension of project files.
pub const PROJECT_EXTENSION: &str = "atomik";

/// An asset the project references that its bundle does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingAsset {
    /// Asset id (content hash).
    pub hash: String,
    /// Display name from the sample pool (the original file name), or the hash.
    pub name: String,
}

fn folder_of(file: &Path) -> &Path {
    file.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The bundle folder of an opened project file (module docs), if it has one.
pub fn existing_bundle_root(file: &Path) -> Option<PathBuf> {
    let folder = folder_of(file);
    let stem = file.file_stem()?;
    if folder.file_name() == Some(stem) {
        return Some(folder.to_path_buf());
    }
    let sibling = folder.join(stem);
    if sibling.is_dir() {
        return Some(sibling);
    }
    folder.join("Audio").is_dir().then(|| folder.to_path_buf())
}

/// Where Save As puts a project **with audio** picked as `picked`: the project file and its
/// bundle folder (module docs).
pub fn save_as_layout(picked: &Path) -> (PathBuf, PathBuf) {
    let folder = folder_of(picked);
    let Some(stem) = picked.file_stem() else {
        return (picked.to_path_buf(), folder.to_path_buf());
    };
    if folder.file_name() == Some(stem) {
        return (picked.to_path_buf(), folder.to_path_buf());
    }
    let root = folder.join(stem);
    let file = root.join(format!("{}.{PROJECT_EXTENSION}", stem.to_string_lossy()));
    (file, root)
}

/// The sibling bundle folder `<folder>/<name>/` of a flat project file `<folder>/<name>.atomik`
/// (where a project that gains audio on a plain Save puts it).
pub fn sibling_bundle_root(file: &Path) -> PathBuf {
    match file.file_stem() {
        Some(stem) => folder_of(file).join(stem),
        None => folder_of(file).to_path_buf(),
    }
}

fn is_project_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION))
}

/// The project file inside a bundle folder: `<folder>/<folder name>.atomik`, else the only
/// `.atomik` file in it.
pub fn project_file_in(folder: &Path) -> Option<PathBuf> {
    if let Some(name) = folder.file_name() {
        let named = folder.join(format!("{}.{PROJECT_EXTENSION}", name.to_string_lossy()));
        if is_project_file(&named) {
            return Some(named);
        }
    }
    let mut found = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| is_project_file(p));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// `path` itself, or, for a folder, the project file inside it ([`project_file_in`]), else
/// the same-named project beside it (the sibling layout `name.atomik` + `name/Audio/`).
pub fn resolve_project_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        project_file_in(path)
            .or_else(|| {
                let name = path.file_name()?;
                let sibling =
                    path.with_file_name(format!("{}.{PROJECT_EXTENSION}", name.to_string_lossy()));
                is_project_file(&sibling).then_some(sibling)
            })
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn pool_name(doc: &RackDoc, hash: &str) -> String {
    doc.assets
        .get(hash)
        .map_or_else(|| hash.to_owned(), |m| m.original_name.clone())
}

/// The assets of `doc`'s sample pool that `bundle` lacks (all of them without a bundle).
pub fn missing_assets(doc: &RackDoc, bundle: Option<&Bundle>) -> Vec<MissingAsset> {
    doc.assets
        .iter()
        .filter(|(hash, _)| !bundle.is_some_and(|b| b.exists(hash)))
        .map(|(hash, meta)| MissingAsset {
            hash: hash.clone(),
            name: meta.original_name.clone(),
        })
        .collect()
}

/// Loads every asset the song references fully into memory (a bounce never streams).
///
/// # Errors
/// A message naming each missing asset (`name (hash)`), or the first unreadable one, or an
/// asset whose sample rate is not the project's.
pub fn load_song_assets(
    doc: &RackDoc,
    bundle: Option<&Bundle>,
) -> Result<BTreeMap<String, SongAsset>, String> {
    load_song_assets_counted(doc, bundle).map(|(assets, _)| assets)
}

/// [`load_song_assets`] that also reports, per asset, the NaN or infinite samples that were read
/// as silence (a file of the bundle that was not written by this program can hold them).
///
/// # Errors
/// As [`load_song_assets`].
pub fn load_song_assets_counted(
    doc: &RackDoc,
    bundle: Option<&Bundle>,
) -> Result<(BTreeMap<String, SongAsset>, Vec<crate::patch::Warning>), String> {
    let mut warnings = Vec::new();
    let wanted = doc.referenced_assets();
    let missing: Vec<String> = wanted
        .iter()
        .filter(|hash| !bundle.is_some_and(|b| b.exists(hash)))
        .map(|hash| format!("{} ({hash})", pool_name(doc, hash)))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "audio asset{} missing from the project bundle: {}",
            if missing.len() == 1 { "" } else { "s" },
            missing.join(", ")
        ));
    }
    let mut out = BTreeMap::new();
    let Some(bundle) = bundle else {
        return Ok((out, warnings));
    };
    for hash in wanted {
        let name = pool_name(doc, &hash);
        let decoded = bundle
            .load_counted(&hash)
            .map_err(|e| format!("audio asset {name} ({hash}): {e}"))?;
        if decoded.replaced > 0 {
            warnings.push(crate::patch::Warning {
                path: format!("assets.{hash}"),
                message: format!(
                    "{name}: {} NaN or infinite samples were read as silence",
                    decoded.replaced
                ),
            });
        }
        let data = decoded.audio;
        if data.sample_rate != doc.sample_rate {
            return Err(format!(
                "audio asset {name} ({hash}) is {} Hz but the project is {} Hz",
                data.sample_rate, doc.sample_rate
            ));
        }
        out.insert(hash, SongAsset::memory(Arc::from(data.channels)));
    }
    Ok((out, warnings))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    pub(crate) struct Tmp(pub PathBuf);

    impl Tmp {
        pub(crate) fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "atomik-render-bundle-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_project_inside_a_folder_named_like_it_is_its_own_bundle() {
        let t = Tmp::new("own");
        let root = t.0.join("song");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("song.atomik");
        assert_eq!(existing_bundle_root(&file), Some(root));
    }

    #[test]
    fn a_flat_project_next_to_a_folder_named_like_it_uses_that_folder() {
        let t = Tmp::new("sibling");
        let file = t.0.join("song.atomik");
        assert_eq!(existing_bundle_root(&file), None);
        fs::create_dir_all(t.0.join("song")).unwrap();
        assert_eq!(existing_bundle_root(&file), Some(t.0.join("song")));
        assert_eq!(sibling_bundle_root(&file), t.0.join("song"));
    }

    #[test]
    fn a_renamed_project_in_a_folder_with_audio_uses_its_folder() {
        let t = Tmp::new("renamed");
        let root = t.0.join("take-1");
        fs::create_dir_all(root.join("Audio")).unwrap();
        let file = root.join("final.atomik");
        assert_eq!(existing_bundle_root(&file), Some(root));
    }

    /// A bundle in `dir` holding one mono 48 kHz asset of `frames` frames (a ramp), and a
    /// document with an audio track playing it.
    pub(crate) fn project_with_asset(
        dir: &Path,
        frames: usize,
        rate: u32,
    ) -> (RackDoc, Bundle, String) {
        use atomik_audio_file::AudioData;
        use atomik_model::{AssetMeta, Command, Document, FadeCurve, SongCommand, empty_doc};
        let mut bundle = Bundle::create(dir).unwrap();
        let ramp: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
        let info = bundle
            .add_recording(&AudioData::new(rate, vec![ramp]).unwrap(), "ramp.wav")
            .unwrap();
        let registry = crate::default_registry();
        let mut d = Document::new(empty_doc());
        let mut apply = |c: Command| {
            d.apply(c, &registry).unwrap();
        };
        apply(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        });
        apply(Command::Song(SongCommand::AddAudioTrack { name: None }));
        apply(Command::Song(SongCommand::AddAudioClip {
            device: atomik_model::DeviceKey("audio-track".into()),
            start: 0,
            length: atomik_core::PPQ,
            asset: info.hash.clone(),
            source_offset: 0,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: FadeCurve::Linear,
        }));
        (d.doc().clone(), bundle, info.hash)
    }

    #[test]
    fn missing_assets_lists_the_pool_assets_the_bundle_lacks() {
        let t = Tmp::new("missing");
        let (doc, bundle, hash) = project_with_asset(&t.0.join("song"), 1000, 48_000);
        assert!(missing_assets(&doc, Some(&bundle)).is_empty());
        std::fs::remove_file(bundle.asset_path(&hash)).unwrap();
        assert_eq!(
            missing_assets(&doc, Some(&bundle)),
            vec![MissingAsset {
                hash: hash.clone(),
                name: "ramp.wav".into()
            }]
        );
        assert_eq!(
            missing_assets(&doc, None).len(),
            1,
            "no bundle: all missing"
        );
    }

    #[test]
    fn song_assets_load_fully_into_memory() {
        let t = Tmp::new("load");
        let (doc, bundle, hash) = project_with_asset(&t.0.join("song"), 1000, 48_000);
        let assets = load_song_assets(&doc, Some(&bundle)).unwrap();
        assert_eq!(assets.len(), 1);
        let SongAsset::Memory {
            data,
            frames,
            channels,
        } = &assets[&hash]
        else {
            panic!("a bounce loads assets into memory");
        };
        assert_eq!((*frames, *channels, data.len()), (1000, 1, 1));
        assert_eq!(data[0], bundle.load(&hash).unwrap().channels[0]);
    }

    #[test]
    fn a_missing_asset_is_an_error_naming_it() {
        let t = Tmp::new("load-missing");
        let (doc, bundle, hash) = project_with_asset(&t.0.join("song"), 1000, 48_000);
        std::fs::remove_file(bundle.asset_path(&hash)).unwrap();
        let e = load_song_assets(&doc, Some(&bundle)).unwrap_err();
        assert!(e.contains("ramp.wav") && e.contains(&hash), "{e}");
        let e = load_song_assets(&doc, None).unwrap_err();
        assert!(e.contains("ramp.wav"), "{e}");
    }

    #[test]
    fn an_asset_at_another_sample_rate_is_an_error() {
        let t = Tmp::new("load-rate");
        let (doc, bundle, _) = project_with_asset(&t.0.join("song"), 1000, 44_100);
        let e = load_song_assets(&doc, Some(&bundle)).unwrap_err();
        assert!(e.contains("44100") && e.contains("48000"), "{e}");
    }

    #[test]
    fn a_project_without_audio_needs_no_bundle() {
        let doc = atomik_model::empty_doc();
        assert!(load_song_assets(&doc, None).unwrap().is_empty());
        assert!(missing_assets(&doc, None).is_empty());
    }

    #[test]
    fn a_plain_project_has_no_bundle() {
        let t = Tmp::new("plain");
        assert_eq!(existing_bundle_root(&t.0.join("song.atomik")), None);
    }

    #[test]
    fn save_as_nests_the_project_in_a_new_folder_unless_it_already_is_in_one() {
        let t = Tmp::new("saveas");
        let picked = t.0.join("song.atomik");
        let (file, root) = save_as_layout(&picked);
        assert_eq!(root, t.0.join("song"));
        assert_eq!(file, t.0.join("song").join("song.atomik"));
        // Picking the project of an existing bundle keeps it where it is.
        let inside = t.0.join("song").join("song.atomik");
        assert_eq!(save_as_layout(&inside), (inside.clone(), t.0.join("song")));
        // A same-named file elsewhere nests again: only the folder name decides.
        let other = t.0.join("other").join("song.atomik");
        assert_eq!(save_as_layout(&other).1, t.0.join("other").join("song"));
    }

    #[test]
    fn a_bundle_folder_resolves_to_its_project_file() {
        let t = Tmp::new("resolve");
        let root = t.0.join("song");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(project_file_in(&root), None);
        fs::write(root.join("a.atomik"), "{}").unwrap();
        assert_eq!(project_file_in(&root), Some(root.join("a.atomik")));
        fs::write(root.join("b.atomik"), "{}").unwrap();
        assert_eq!(project_file_in(&root), None, "two candidates: ambiguous");
        fs::write(root.join("song.atomik"), "{}").unwrap();
        assert_eq!(project_file_in(&root), Some(root.join("song.atomik")));
        assert_eq!(resolve_project_path(&root), root.join("song.atomik"));
        let file = root.join("a.atomik");
        assert_eq!(resolve_project_path(&file), file);
    }

    #[test]
    fn a_sibling_bundle_folder_resolves_to_the_project_next_to_it() {
        // The examples' layout: `demo.atomik` beside `demo/Audio/`.
        let t = Tmp::new("sibling");
        let root = t.0.join("demo");
        fs::create_dir_all(root.join("Audio")).unwrap();
        fs::write(t.0.join("demo.atomik"), "{}").unwrap();
        assert_eq!(resolve_project_path(&root), t.0.join("demo.atomik"));
        // A trailing separator names the same folder.
        let slash = PathBuf::from(format!("{}{}", root.display(), std::path::MAIN_SEPARATOR));
        assert_eq!(resolve_project_path(&slash), t.0.join("demo.atomik"));
    }
}
