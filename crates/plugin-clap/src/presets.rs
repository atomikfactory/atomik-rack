//! CLAP preset discovery: asking a plugin file which presets it offers and where they live, and
//! loading one through the plugin's `preset-load` extension.
//!
//! A file may hold several providers. Each declares file types and locations (a folder or a
//! file, or the plugin itself); the host walks the folders for files of the declared types and
//! asks the provider for the metadata of each location. Only presets that name this plugin
//! (or name none) are kept. Every preset gets a short id made from where it lives, so an id
//! stays the same from one listing to the next and fits the sandbox wire; loading looks the id
//! up in the listing it came from, so the plugin is only ever asked for a location it declared.

use atomik_plugin_host::{BackendError, PresetInfo};
use clack_extensions::preset_discovery::PluginPresetLoad;
use clack_extensions::preset_discovery::prelude::*;
use clack_host::prelude::*;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

/// Most providers read from one file.
const MAX_PROVIDERS: usize = 16;
/// Most locations one provider declares that are read.
const MAX_LOCATIONS: usize = 64;
/// Most files read under all the folders of one provider.
const MAX_FILES: usize = 5_000;
/// Folders deeper than this under a declared location are not read.
const MAX_DEPTH: usize = 6;
/// Most presets one listing returns (the sandbox wire cuts at 2000 too).
pub(crate) const MAX_PRESETS: usize = 2_000;

/// Where a preset lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// Inside the plugin.
    Plugin,
    /// In a file.
    File(CString),
}

/// What loading a preset needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    place: Place,
    key: Option<CString>,
}

/// The presets of one plugin: what the user sees and what each id loads.
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    pub list: Vec<PresetInfo>,
    targets: BTreeMap<String, Target>,
}

impl Catalog {
    /// The target of the preset `id`, when the listing holds it.
    pub fn target(&self, id: &str) -> Option<&Target> {
        self.targets.get(id)
    }

    fn push(&mut self, name: String, group: String, target: Target) {
        if self.list.len() >= MAX_PRESETS {
            return;
        }
        let mut id = id_of(&target, &name);
        while self.targets.contains_key(&id) {
            id.push('x');
        }
        self.list.push(PresetInfo {
            id: id.clone(),
            name,
            group,
        });
        self.targets.insert(id, target);
    }
}

/// A short stable id for a preset (FNV-1a over where it lives and its name).
fn id_of(target: &Target, name: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes.iter().chain(std::iter::once(&0)) {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    match &target.place {
        Place::Plugin => feed(b"plugin"),
        Place::File(path) => feed(path.as_bytes()),
    }
    feed(target.key.as_deref().map_or(&[][..], CStr::to_bytes));
    feed(name.as_bytes());
    format!("c{hash:016x}")
}

fn text(s: &CStr) -> String {
    s.to_string_lossy().into_owned()
}

/// What a provider declared while it started.
#[derive(Default)]
struct Declared {
    /// File extensions to match (lower case, no dot); an empty entry matches every file.
    extensions: Vec<String>,
    /// `(name, place)` of each location.
    locations: Vec<(String, Place)>,
}

impl IndexerImpl for Declared {
    fn declare_filetype(&mut self, file_type: FileType) -> Result<(), HostError> {
        let ext = file_type
            .file_extension
            .map(|e| text(e).trim_start_matches('.').to_ascii_lowercase())
            .unwrap_or_default();
        if self.extensions.len() < 64 {
            self.extensions.push(ext);
        }
        Ok(())
    }

    fn declare_location(&mut self, location: LocationInfo) -> Result<(), HostError> {
        let place = match location.location {
            Location::Plugin => Place::Plugin,
            Location::File { path } => Place::File(path.to_owned()),
        };
        if self.locations.len() < MAX_LOCATIONS {
            self.locations.push((text(location.name), place));
        }
        Ok(())
    }

    fn declare_soundpack(&mut self, _soundpack: Soundpack) -> Result<(), HostError> {
        Ok(())
    }
}

/// One preset a provider described, while it is being described.
#[derive(Default)]
struct Draft {
    name: Option<String>,
    key: Option<CString>,
    /// Plugin ids the preset names; empty when it names none.
    plugins: Vec<String>,
}

/// Collects the presets of one location.
struct Receiver<'a> {
    plugin_id: &'a str,
    place: Place,
    group: &'a str,
    fallback_name: String,
    current: Option<Draft>,
    out: &'a mut Catalog,
}

impl Receiver<'_> {
    /// Files the preset being described, when it is for this plugin.
    fn finish(&mut self) {
        let Some(draft) = self.current.take() else {
            return;
        };
        if !draft.plugins.is_empty() && !draft.plugins.iter().any(|p| p == self.plugin_id) {
            return;
        }
        let name = draft
            .name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.fallback_name.clone());
        let target = Target {
            place: self.place.clone(),
            key: draft.key,
        };
        self.out.push(name, self.group.to_string(), target);
    }
}

impl MetadataReceiverImpl for Receiver<'_> {
    fn on_error(&mut self, _error_code: i32, _error_message: Option<&CStr>) {}

    fn begin_preset(
        &mut self,
        name: Option<&CStr>,
        load_key: Option<&CStr>,
    ) -> Result<(), HostError> {
        self.finish();
        self.current = Some(Draft {
            name: name.map(text),
            key: load_key.map(CStr::to_owned),
            plugins: Vec::new(),
        });
        Ok(())
    }

    fn add_plugin_id(&mut self, plugin_id: UniversalPluginId) {
        if let Some(d) = &mut self.current {
            // Only CLAP ids name this plugin; an id of another format is for another build.
            if plugin_id.abi.to_bytes().eq_ignore_ascii_case(b"clap") && d.plugins.len() < 32 {
                d.plugins.push(text(plugin_id.id));
            }
        }
    }

    fn set_soundpack_id(&mut self, _soundpack_id: &CStr) {}
    fn set_flags(&mut self, _flags: Flags) {}
    fn add_creator(&mut self, _creator: &CStr) {}
    fn set_description(&mut self, _description: &CStr) {}
    fn set_timestamps(&mut self, _created: Option<Timestamp>, _modified: Option<Timestamp>) {}
    fn add_feature(&mut self, _feature: &CStr) {}
    fn add_extra_info(&mut self, _key: &CStr, _value: &CStr) {}
}

/// The files under `root` whose extension is in `extensions` (an empty entry matches all), or
/// `root` itself when it is a file. Sorted, bounded in count and depth.
fn files_under(root: &Path, extensions: &[String], budget: &mut usize) -> Vec<PathBuf> {
    let matches = |p: &Path| {
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        extensions.is_empty() || extensions.iter().any(|e| e.is_empty() || *e == ext)
    };
    let mut found = Vec::new();
    if root.is_file() {
        if matches(root) {
            found.push(root.to_path_buf());
        }
        return found;
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for path in entries {
            if *budget == 0 {
                return found;
            }
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if matches(&path) {
                *budget -= 1;
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn cstring(path: &Path) -> Option<CString> {
    CString::new(path.to_string_lossy().into_owned()).ok()
}

/// Every preset of the plugin `plugin_id` the file behind `entry` offers.
pub(crate) fn discover(entry: &PluginEntry, host_info: &HostInfo, plugin_id: &str) -> Catalog {
    let mut catalog = Catalog::default();
    let Some(factory) = entry.get_factory::<PresetDiscoveryFactory>() else {
        return catalog;
    };
    for descriptor in factory.provider_descriptors().take(MAX_PROVIDERS) {
        let Some(provider_id) = descriptor.id() else {
            continue;
        };
        let Ok(mut provider) =
            Provider::instantiate(Declared::default(), entry, provider_id, host_info)
        else {
            continue;
        };
        let declared = std::mem::take(provider.indexer_mut());
        let mut files_left = MAX_FILES;
        for (name, place) in &declared.locations {
            // The places this location stands for: the plugin, or each matching file.
            let places: Vec<Place> = match place {
                Place::Plugin => vec![Place::Plugin],
                Place::File(path) => {
                    let root = PathBuf::from(path.to_string_lossy().into_owned());
                    files_under(&root, &declared.extensions, &mut files_left)
                        .iter()
                        .filter_map(|f| cstring(f))
                        .map(Place::File)
                        .collect()
                }
            };
            for place in places {
                let fallback = match &place {
                    Place::Plugin => name.clone(),
                    Place::File(p) => Path::new(&*p.to_string_lossy())
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                };
                let mut receiver = Receiver {
                    plugin_id,
                    place: place.clone(),
                    group: name,
                    fallback_name: fallback,
                    current: None,
                    out: &mut catalog,
                };
                let location = match &place {
                    Place::Plugin => Location::Plugin,
                    Place::File(path) => Location::File { path },
                };
                provider.get_metadata(location, &mut receiver);
                receiver.finish();
            }
        }
    }
    catalog
}

/// Loads the preset behind `target` into the plugin.
pub(crate) fn load(
    plugin: &mut clack_host::plugin::PluginInstance<crate::hub::ClapHost>,
    target: &Target,
) -> Result<(), BackendError> {
    let handle = plugin.plugin_handle();
    let ext = handle.get_extension::<PluginPresetLoad>().ok_or_else(|| {
        BackendError::Unsupported("the plugin cannot load the presets it lists".to_string())
    })?;
    let location = match &target.place {
        Place::Plugin => Location::Plugin,
        Place::File(path) => Location::File { path },
    };
    ext.load_from_location(&handle, location, target.key.as_deref())
        .map_err(|e| BackendError::Failed(format!("loading the preset: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str, key: Option<&str>) -> Target {
        Target {
            place: Place::File(CString::new(path).unwrap()),
            key: key.map(|k| CString::new(k).unwrap()),
        }
    }

    #[test]
    fn ids_are_short_and_stable_and_tell_places_apart() {
        let a = id_of(&target("/p/a.pre", None), "A");
        assert_eq!(a, id_of(&target("/p/a.pre", None), "A"));
        assert_ne!(a, id_of(&target("/p/b.pre", None), "A"));
        assert_ne!(a, id_of(&target("/p/a.pre", Some("1")), "A"));
        assert_ne!(
            a,
            id_of(
                &Target {
                    place: Place::Plugin,
                    key: None
                },
                "A"
            )
        );
        assert!(a.len() <= 17 && a.starts_with('c'));
    }

    #[test]
    fn two_presets_with_one_id_both_stay_loadable() {
        let mut c = Catalog::default();
        let t = target("/p/a.pre", None);
        c.push("A".into(), String::new(), t.clone());
        c.push("A".into(), String::new(), t);
        assert_eq!(c.list.len(), 2);
        assert_ne!(c.list[0].id, c.list[1].id);
        assert!(c.target(&c.list[1].id).is_some());
        assert!(c.target("c-not-listed").is_none());
    }

    #[test]
    fn the_folder_walk_matches_extensions_and_is_bounded() {
        let dir = std::env::temp_dir().join(format!("atomik-clap-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["a.pre", "b.txt", "sub/c.PRE"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut budget = 10;
        let found = files_under(&dir, &["pre".to_string()], &mut budget);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(budget, 8);
        let mut budget = 1;
        assert_eq!(files_under(&dir, &[String::new()], &mut budget).len(), 1);
        let single = files_under(&dir.join("b.txt"), &["pre".to_string()], &mut 5);
        assert!(single.is_empty(), "a file of another type is not a preset");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
