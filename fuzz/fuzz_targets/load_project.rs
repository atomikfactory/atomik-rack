//! A project file: any text must load or be refused, never panic or run away. What loads is
//! saved again, which must not panic either.
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
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok((doc, _warnings)) = atomik_model::load_project_with_warnings(text, registry()) {
        let _ = atomik_model::save_project(&doc);
    }
});
