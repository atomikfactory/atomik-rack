//! Preset discovery and `preset-load` for the test plugins: a library lists [`PresetDef`]s, and
//! the shell serves them through a `clap.preset-discovery-factory` with one provider, then loads
//! them through the plugin's `clap.preset-load` extension.
//!
//! Besides the built-in presets (location kind plugin) the provider declares a folder of
//! `.tpreset` files when the environment variable [`PRESET_DIR_ENV`] names one: a file holds the
//! CLAP id of its plugin on the first line and one `param id=value` per line after it.

use super::{LibraryDef, instance, lock};
use clap_sys::ext::preset_load::clap_plugin_preset_load;
use clap_sys::factory::preset_discovery::{
    CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT, CLAP_PRESET_DISCOVERY_IS_USER_CONTENT,
    CLAP_PRESET_DISCOVERY_LOCATION_FILE, CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN,
    clap_preset_discovery_filetype, clap_preset_discovery_indexer, clap_preset_discovery_location,
    clap_preset_discovery_metadata_receiver, clap_preset_discovery_provider,
    clap_preset_discovery_provider_descriptor,
};
use clap_sys::plugin::clap_plugin;
use clap_sys::universal_plugin_id::clap_universal_plugin_id;
use clap_sys::version::CLAP_VERSION;
use std::ffi::{CStr, CString, c_char, c_void};

/// One preset a library offers through preset discovery: the plugin it is for, its name, the key
/// `preset-load` gets back and the param values loading it sets.
#[derive(Debug)]
pub struct PresetDef {
    /// The CLAP id of the plugin the preset is for.
    pub plugin: &'static CStr,
    /// The name to show.
    pub name: &'static CStr,
    /// The load key.
    pub key: &'static CStr,
    /// `(param id, value)` pairs the preset sets.
    pub values: &'static [(u32, f64)],
}

/// The environment variable naming a folder of `.tpreset` files the library also declares as a
/// preset location.
pub const PRESET_DIR_ENV: &str = "ATOMIK_TEST_CLAP_PRESET_DIR";

fn preset_dir() -> Option<CString> {
    std::env::var(PRESET_DIR_ENV)
        .ok()
        .and_then(|d| CString::new(d).ok())
}

/// Whether a folder of `.tpreset` files is declared.
pub(crate) fn dir_set() -> bool {
    preset_dir().is_some()
}

/// The plugin id and the values of a `.tpreset` file.
fn parse_preset_file(text: &str) -> Option<(String, Vec<(u32, f64)>)> {
    let mut lines = text.lines();
    let plugin = lines.next()?.trim().to_string();
    let values = lines
        .filter_map(|l| {
            let (id, v) = l.split_once('=')?;
            Some((id.trim().parse().ok()?, v.trim().parse().ok()?))
        })
        .collect();
    Some((plugin, values))
}

/// Whether the library offers presets at all (and so serves the factory and the extension).
pub(crate) fn offered(lib: &LibraryDef) -> bool {
    !lib.presets.is_empty() || preset_dir().is_some()
}

#[repr(C)]
struct ProviderObj {
    raw: clap_preset_discovery_provider,
    lib: &'static LibraryDef,
    indexer: *const clap_preset_discovery_indexer,
}

static PROVIDER_DESC: clap_preset_discovery_provider_descriptor =
    clap_preset_discovery_provider_descriptor {
        clap_version: CLAP_VERSION,
        id: c"atomik.test.presets".as_ptr(),
        name: c"Atomik test presets".as_ptr(),
        vendor: c"Atomik".as_ptr(),
    };

/// Preset factory `count`.
pub fn count() -> u32 {
    1
}

/// Preset factory `get_descriptor`.
pub fn descriptor(index: u32) -> *const clap_preset_discovery_provider_descriptor {
    if index == 0 {
        &raw const PROVIDER_DESC
    } else {
        std::ptr::null()
    }
}

/// Preset factory `create`.
pub fn create(
    lib: &'static LibraryDef,
    indexer: *const clap_preset_discovery_indexer,
    id: *const c_char,
) -> *const clap_preset_discovery_provider {
    if id.is_null() || indexer.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the host passes a NUL-terminated provider id.
    if unsafe { CStr::from_ptr(id) } != c"atomik.test.presets" {
        return std::ptr::null();
    }
    let obj = Box::new(ProviderObj {
        raw: clap_preset_discovery_provider {
            desc: &raw const PROVIDER_DESC,
            provider_data: std::ptr::null_mut(),
            init: Some(provider_init),
            destroy: Some(provider_destroy),
            get_metadata: Some(provider_get_metadata),
            get_extension: Some(provider_get_extension),
        },
        lib,
        indexer,
    });
    let raw = Box::into_raw(obj);
    // SAFETY: `raw` comes from `Box::into_raw` just above and is not shared yet.
    unsafe {
        (*raw).raw.provider_data = raw.cast();
        &raw const (*raw).raw
    }
}

fn provider<'a>(p: *const clap_preset_discovery_provider) -> &'a ProviderObj {
    // SAFETY: the host only calls a provider with the pointer `create` returned, whose
    // `provider_data` points to the live boxed `ProviderObj` until `destroy`.
    unsafe { &*(*p).provider_data.cast::<ProviderObj>() }
}

unsafe extern "C" fn provider_init(p: *const clap_preset_discovery_provider) -> bool {
    let obj = provider(p);
    let indexer = obj.indexer;
    // SAFETY: the indexer of the host is valid for the life of the provider (CLAP contract).
    let (declare_filetype, declare_location) =
        unsafe { ((*indexer).declare_filetype, (*indexer).declare_location) };
    let (Some(declare_filetype), Some(declare_location)) = (declare_filetype, declare_location)
    else {
        return false;
    };
    let file_type = clap_preset_discovery_filetype {
        name: c"Test preset".as_ptr(),
        description: std::ptr::null(),
        file_extension: c"tpreset".as_ptr(),
    };
    // SAFETY: valid structs; the host copies what it keeps.
    unsafe { declare_filetype(indexer, &file_type) };
    if !obj.lib.presets.is_empty() {
        let factory = clap_preset_discovery_location {
            flags: CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT,
            name: c"Factory".as_ptr(),
            kind: CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN,
            location: std::ptr::null(),
        };
        // SAFETY: as above.
        unsafe { declare_location(indexer, &factory) };
    }
    if let Some(dir) = preset_dir() {
        let user = clap_preset_discovery_location {
            flags: CLAP_PRESET_DISCOVERY_IS_USER_CONTENT,
            name: c"User".as_ptr(),
            kind: CLAP_PRESET_DISCOVERY_LOCATION_FILE,
            location: dir.as_ptr(),
        };
        // SAFETY: as above; `dir` outlives the call.
        unsafe { declare_location(indexer, &user) };
    }
    true
}

unsafe extern "C" fn provider_destroy(p: *const clap_preset_discovery_provider) {
    // SAFETY: `destroy` is the last call on the provider; the pointer came from `Box::into_raw`
    // in `create` and is not used again.
    drop(unsafe { Box::from_raw((*p).provider_data.cast::<ProviderObj>()) });
}

unsafe extern "C" fn provider_get_extension(
    _p: *const clap_preset_discovery_provider,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

/// Tells `receiver` about one preset of `plugin`.
fn announce(
    receiver: *const clap_preset_discovery_metadata_receiver,
    name: &CStr,
    key: Option<&CStr>,
    plugin: &CStr,
) {
    // SAFETY: the receiver of the host is valid for the call (CLAP contract) and the strings
    // outlive it.
    unsafe {
        let r = &*receiver;
        if let Some(begin) = r.begin_preset {
            begin(
                receiver,
                name.as_ptr(),
                key.map_or(std::ptr::null(), CStr::as_ptr),
            );
        }
        if let Some(add) = r.add_plugin_id {
            let id = clap_universal_plugin_id {
                abi: c"clap".as_ptr(),
                id: plugin.as_ptr(),
            };
            add(receiver, &id);
        }
    }
}

unsafe extern "C" fn provider_get_metadata(
    p: *const clap_preset_discovery_provider,
    kind: u32,
    location: *const c_char,
    receiver: *const clap_preset_discovery_metadata_receiver,
) -> bool {
    let obj = provider(p);
    if kind == CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN {
        for def in obj.lib.presets {
            announce(receiver, def.name, Some(def.key), def.plugin);
        }
        return true;
    }
    if location.is_null() {
        return false;
    }
    // SAFETY: the host passes a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(location) }
        .to_string_lossy()
        .into_owned();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Some((plugin, _)) = parse_preset_file(&text) else {
        return false;
    };
    let (Ok(plugin), Some(stem)) = (
        CString::new(plugin),
        std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| CString::new(s.to_string_lossy().into_owned()).ok()),
    ) else {
        return false;
    };
    announce(receiver, &stem, None, &plugin);
    true
}

unsafe extern "C" fn from_location(
    plugin: *const clap_plugin,
    kind: u32,
    location: *const c_char,
    key: *const c_char,
) -> bool {
    let i = instance(plugin);
    let values: Vec<(u32, f64)> = if kind == CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN {
        if key.is_null() {
            return false;
        }
        // SAFETY: the host passes a NUL-terminated key.
        let key = unsafe { CStr::from_ptr(key) };
        let Some(def) = i
            .presets
            .iter()
            .find(|d| d.plugin == i.info.id && d.key == key)
        else {
            return false;
        };
        def.values.to_vec()
    } else {
        if location.is_null() {
            return false;
        }
        // SAFETY: the host passes a NUL-terminated path.
        let path = unsafe { CStr::from_ptr(location) }
            .to_string_lossy()
            .into_owned();
        let Some((owner, values)) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| parse_preset_file(&t))
        else {
            return false;
        };
        if owner.as_bytes() != i.info.id.to_bytes() {
            return false;
        }
        values
    };
    let mut inner = lock(i);
    for (id, value) in values {
        inner.dsp.set_param(id, value);
    }
    true
}

/// The `clap.preset-load` table of every plugin of a library that offers presets.
pub(crate) static PRESET_LOAD: clap_plugin_preset_load = clap_plugin_preset_load {
    from_location: Some(from_location),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_file_names_its_plugin_and_its_values() {
        let (plugin, values) = parse_preset_file("atomik.test.gain\n1=-6.5\nbad\n2 = 1\n").unwrap();
        assert_eq!(plugin, "atomik.test.gain");
        assert_eq!(values, vec![(1, -6.5), (2, 1.0)]);
        assert!(parse_preset_file("").is_none());
    }
}
