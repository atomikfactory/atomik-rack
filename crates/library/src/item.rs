//! Library items: sections, device URIs, unique names and item info.
//!
//! Pure string and path logic. The app writes and reads the item files; this module only
//! names them and says which rows belong in which section of the browser.

use crate::FileKind;
use std::path::{Path, PathBuf};

/// Extension of a patch item (devices and the cables between them).
pub const PATCH_EXT: &str = "atomikpatch";
/// Extension of a container item (one container device with its inner devices).
pub const CONTAINER_EXT: &str = "atomikcontainer";
/// Extension of a note clip item.
pub const CLIP_EXT: &str = "atomikclip";
/// Extension of a project file.
pub const PROJECT_EXT: &str = "atomik";
/// Scheme of the virtual path of a built-in device row. Never a file system path.
pub const DEVICE_URI_PREFIX: &str = "atomik-device:";

/// The sections of the library in the browser tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LibrarySection {
    /// Built-in devices, from the device registry (virtual rows).
    Devices,
    /// Patch items.
    Patches,
    /// Container items.
    Containers,
    /// Note clip items.
    Clips,
    /// Project files.
    Projects,
}

impl LibrarySection {
    /// Every section, in tree order.
    pub const ALL: [LibrarySection; 5] = [
        LibrarySection::Devices,
        LibrarySection::Patches,
        LibrarySection::Containers,
        LibrarySection::Clips,
        LibrarySection::Projects,
    ];

    /// The section's name in the tree.
    pub fn title(self) -> &'static str {
        match self {
            Self::Devices => "Devices",
            Self::Patches => "Patches",
            Self::Containers => "Containers",
            Self::Clips => "Clips",
            Self::Projects => "Projects",
        }
    }

    /// The sub-folder of the library folder that holds the section's files, or `None` for
    /// the virtual device section.
    pub fn folder_name(self) -> Option<&'static str> {
        match self {
            Self::Devices => None,
            other => Some(other.title()),
        }
    }

    /// The section a kind of row is listed in. Folders and audio belong to none.
    pub fn of(kind: FileKind) -> Option<Self> {
        match kind {
            FileKind::Device => Some(Self::Devices),
            FileKind::Patch => Some(Self::Patches),
            FileKind::Container => Some(Self::Containers),
            FileKind::Clip => Some(Self::Clips),
            FileKind::Project => Some(Self::Projects),
            FileKind::Folder | FileKind::Audio(_) => None,
        }
    }

    /// Whether a file of `kind` is listed in this section.
    pub fn accepts(self, kind: FileKind) -> bool {
        Self::of(kind) == Some(self)
    }
}

/// The virtual path of the built-in device `type_id`.
pub fn device_uri(type_id: &str) -> PathBuf {
    PathBuf::from(format!("{DEVICE_URI_PREFIX}{type_id}"))
}

/// The device type id of a virtual device path, or `None` for any other path.
pub fn device_type_of(path: &Path) -> Option<&str> {
    path.to_str()?
        .strip_prefix(DEVICE_URI_PREFIX)
        .filter(|id| !id.is_empty())
}

/// The first free file name `stem.ext`, `stem 2.ext`, `stem 3.ext`, ... in `dir`, where
/// `exists` says whether a path is taken (the caller owns the disk access).
pub fn unique_name(dir: &Path, stem: &str, ext: &str, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !exists(&first) {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{stem} {n}.{ext}")))
        .find(|p| !exists(p))
        .unwrap_or(first)
}

/// What the probe worker learned from an item or a device: the detail column text and the
/// category the search field also matches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemInfo {
    /// The detail column, such as "3 devices: Poly Synth, Delay, Reverb".
    pub detail: String,
    /// The group the item belongs to (a device kind label), possibly empty.
    pub category: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioFormat, classify};
    use std::collections::BTreeSet;

    #[test]
    fn classify_item_extensions_any_case_gives_the_item_kinds() {
        let cases = [
            ("a.atomikpatch", FileKind::Patch),
            ("a.ATOMIKPATCH", FileKind::Patch),
            ("a.atomikcontainer", FileKind::Container),
            ("a.atomikclip", FileKind::Clip),
            ("a.AtomikClip", FileKind::Clip),
            ("a.atomik", FileKind::Project),
            ("a.ATOMIK", FileKind::Project),
        ];
        for (name, kind) in cases {
            assert_eq!(classify(Path::new(name), false), Some(kind), "{name}");
        }
    }

    #[test]
    fn classify_near_misses_are_not_items() {
        for name in [
            "a.atomikpatch.bak",
            "a.atomiks",
            "atomik",
            ".atomik",
            "a.json",
        ] {
            assert_eq!(classify(Path::new(name), false), None, "{name}");
        }
        assert_eq!(
            classify(Path::new("a.wav"), false),
            Some(FileKind::Audio(AudioFormat::Wav))
        );
    }

    #[test]
    fn classify_a_folder_named_like_an_item_is_a_folder() {
        assert_eq!(
            classify(Path::new("x.atomik"), true),
            Some(FileKind::Folder)
        );
    }

    #[test]
    fn section_accepts_only_its_own_kind() {
        assert!(LibrarySection::Patches.accepts(FileKind::Patch));
        assert!(!LibrarySection::Patches.accepts(FileKind::Container));
        assert!(LibrarySection::Devices.accepts(FileKind::Device));
        assert!(!LibrarySection::Clips.accepts(FileKind::Folder));
        assert!(!LibrarySection::Projects.accepts(FileKind::Audio(AudioFormat::Wav)));
        for kind in [
            FileKind::Device,
            FileKind::Patch,
            FileKind::Container,
            FileKind::Clip,
            FileKind::Project,
        ] {
            let n = LibrarySection::ALL
                .iter()
                .filter(|s| s.accepts(kind))
                .count();
            assert_eq!(n, 1, "{kind:?}");
        }
    }

    #[test]
    fn folder_name_is_the_title_except_for_devices() {
        assert_eq!(LibrarySection::Devices.folder_name(), None);
        assert_eq!(LibrarySection::Patches.folder_name(), Some("Patches"));
        assert_eq!(LibrarySection::Containers.folder_name(), Some("Containers"));
        assert_eq!(LibrarySection::Clips.folder_name(), Some("Clips"));
        assert_eq!(LibrarySection::Projects.folder_name(), Some("Projects"));
    }

    #[test]
    fn device_uri_round_trips_through_device_type_of() {
        let uri = device_uri("poly-synth");
        assert_eq!(device_type_of(&uri), Some("poly-synth"));
        assert_eq!(uri.to_str(), Some("atomik-device:poly-synth"));
    }

    #[test]
    fn device_type_of_other_paths_is_none() {
        assert_eq!(device_type_of(Path::new("C:/x/kick.wav")), None);
        assert_eq!(device_type_of(Path::new("atomik-device:")), None);
        assert_eq!(device_type_of(Path::new("xatomik-device:a")), None);
    }

    #[test]
    fn unique_name_free_name_is_kept() {
        let taken = |_: &Path| false;
        assert_eq!(
            unique_name(Path::new("lib"), "Poly Synth", "atomikpatch", &taken),
            Path::new("lib").join("Poly Synth.atomikpatch")
        );
    }

    #[test]
    fn unique_name_taken_names_count_up_from_two() {
        let used: BTreeSet<PathBuf> = ["Poly Synth.atomikpatch", "Poly Synth 2.atomikpatch"]
            .iter()
            .map(|n| Path::new("lib").join(n))
            .collect();
        let taken = |p: &Path| used.contains(p);
        assert_eq!(
            unique_name(Path::new("lib"), "Poly Synth", "atomikpatch", &taken),
            Path::new("lib").join("Poly Synth 3.atomikpatch")
        );
    }
}
