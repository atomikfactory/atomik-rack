//! The plugin views are platform-independent: `atomik-ui` has no
//! window-system, file-system, process or plugin-format code in its plugin modules and no
//! dependency on the plugin host or a backend (the host crate is a dev-dependency that only
//! pins the UI's mirrors of its consent text and modes).

use std::path::Path;

const PLUGIN_SOURCES: [&str; 9] = [
    "src/plugin_data.rs",
    "src/plugin_panel.rs",
    "src/plugin_params.rs",
    "src/plugin_params/paint.rs",
    "src/plugin_settings.rs",
    "src/plugin_banner.rs",
    "src/rack/plugin_input.rs",
    "src/browser.rs",
    "src/rack/container_input.rs",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_plugin_modules_use_no_platform_or_plugin_format_code() {
    // Test modules may use anything; only the code above `#[cfg(test)]` counts.
    let forbidden = [
        "winit",
        "raw_window_handle",
        "raw-window-handle",
        "std::fs",
        "std::process",
        "std::thread",
        "libloading",
        "clack",
        "clap_sys",
        "vst3",
        "windows::",
        "unsafe ",
        "atomik_plugin_host",
        "atomik_plugin_clap",
        "SystemTime",
        "Instant",
    ];
    for rel in PLUGIN_SOURCES {
        let text = read(rel);
        let code = text.split("#[cfg(test)]").next().unwrap_or(&text);
        // Doc comments may name things; strip comment lines.
        let code: String = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for word in forbidden {
            assert!(!code.contains(word), "{rel} contains `{word}`");
        }
    }
}

#[test]
fn the_ui_crate_depends_on_no_plugin_crate_outside_dev_dependencies() {
    let manifest = read("Cargo.toml");
    let deps = manifest
        .split("[dev-dependencies]")
        .next()
        .unwrap_or(&manifest);
    assert!(
        !deps.contains("plugin"),
        "[dependencies] names a plugin crate"
    );
    let dev = manifest.split("[dev-dependencies]").nth(1).unwrap_or("");
    assert!(dev.contains("atomik-plugin-host"));
    assert!(!dev.contains("atomik-plugin-clap"));
}
