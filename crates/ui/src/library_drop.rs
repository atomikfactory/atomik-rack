//! Which dropped paths are library items: what the rack and the arrangement take from a file
//! hover or drop (from the operating system or from the file browser's own drag).

use atomik_library::{FileKind, classify, device_type_of};
use std::path::{Path, PathBuf};

/// Whether `path` is a library item or a device URI (anything the library lists that is not
/// a folder or an audio file).
pub(crate) fn is_library(path: &Path) -> bool {
    device_type_of(path).is_some() || classify(path, false).is_some_and(FileKind::is_item)
}

/// The first path the rack can insert: a device URI, a patch or a container item.
pub(crate) fn rack_item(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths.iter().find(|p| {
        device_type_of(p).is_some()
            || matches!(
                classify(p, false),
                Some(FileKind::Patch | FileKind::Container)
            )
    })
}

/// The first patch item (a `.atomikpatch` file).
pub(crate) fn patch_item(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths
        .iter()
        .find(|p| classify(p, false) == Some(FileKind::Patch))
}

/// The device type a patch file holds, when it holds exactly one device (what decides whether
/// the patch can be loaded into a device it is dropped on). Reads the file, so callers cache it.
pub(crate) fn patch_device_type(path: &Path) -> Option<String> {
    let text = atomik_model::read_document_text(path).ok()?;
    atomik_model::summarize_item(atomik_model::ItemKind::Patch, &text)?.device_type
}

/// The first note clip item.
pub(crate) fn clip_item(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths
        .iter()
        .find(|p| classify(p, false) == Some(FileKind::Clip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn is_library_covers_items_and_device_uris_but_not_audio_or_folders() {
        for yes in [
            "a.atomikpatch",
            "A.ATOMIKCONTAINER",
            "a.atomikclip",
            "a.atomik",
            "atomik-device:atomik.poly-synth",
        ] {
            assert!(is_library(Path::new(yes)), "{yes}");
        }
        for no in ["a.wav", "a.mid", "a.json", "atomik-device:", "folder"] {
            assert!(!is_library(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn rack_item_takes_patches_containers_and_devices_only() {
        let all = paths(&["a.wav", "b.atomikclip", "c.atomik", "d.atomikcontainer"]);
        assert_eq!(rack_item(&all), Some(&all[3]));
        let device = paths(&["atomik-device:atomik.delay", "e.atomikpatch"]);
        assert_eq!(rack_item(&device), Some(&device[0]));
        assert_eq!(
            rack_item(&paths(&["a.wav", "b.atomikclip", "c.atomik"])),
            None
        );
        assert_eq!(rack_item(&[]), None);
    }

    #[test]
    fn clip_item_takes_the_first_clip_only() {
        let all = paths(&["a.atomikpatch", "b.atomikclip", "c.atomikclip"]);
        assert_eq!(clip_item(&all), Some(&all[1]));
        assert_eq!(clip_item(&paths(&["a.atomikpatch", "x.wav"])), None);
    }
}
