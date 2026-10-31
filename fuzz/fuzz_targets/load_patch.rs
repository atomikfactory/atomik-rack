//! The smaller documents: a patch, a clip file, a library item (a device, a chain or a
//! container), a MIDI mapping preset and the library database of the file browser. The first
//! byte picks the reader, the rest is the text.
#![no_main]

use std::sync::OnceLock;

use atomik_device_sdk::Registry;
use libfuzzer_sys::fuzz_target;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        atomik_devices::register_all(&mut registry).expect("the built-in devices register");
        registry
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    match which % 5 {
        0 => drop(atomik_model::load_patch(text, registry())),
        1 => drop(atomik_model::load_clip_file(text)),
        2 => drop(atomik_model::load_device_item(text, registry())),
        3 => drop(atomik_model::load_midi_preset(text)),
        _ => drop(atomik_library::LibraryDb::from_json(text)),
    }
});
