//! The sandbox half of the bounce criterion (AC-3): `atomik-render --plugin-sandbox on` writes the
//! same WAV bytes as `--plugin-sandbox off`, for the CLAP demo and the CLAP + VST3 demo, in
//! `--plugin-mode offline` and `realtime`.
//!
//! These run the real `atomik-render` binary, which finds `atomik-plugin-host` beside itself.
//! When that program has not been built (`cargo build -p atomik-plugin-hostd`) the tests print a
//! note and pass, so a plain `cargo test -p atomik-render` stays green.

use atomik_plugin_host::PluginCache;
use atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin};
use atomik_render::plugins::{host_program_name, scan_in_process};
use std::path::{Path, PathBuf};
use std::process::Command;

fn render_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_atomik-render"))
}

/// True when the child program sits beside `atomik-render`; else prints why the test is skipped.
fn child_is_built() -> bool {
    let child = render_exe().with_file_name(host_program_name());
    let built = child.is_file();
    if !built {
        eprintln!(
            "skipped: {} is not built (cargo build -p atomik-plugin-hostd)",
            child.display()
        );
    }
    built
}

fn project(dir: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects")
        .join(dir)
        .join(file)
}

fn test_cache() -> PluginCache {
    let clap = clap_test_plugin("atomik-test-clap").expect("the CLAP test plugin builds");
    let vst3 = vst3_test_plugin("atomik-test-vst3").expect("the VST3 test plugin builds");
    scan_in_process(&[clap, vst3])
}

/// Bounces `project` with the given sandbox switch and plugin mode; returns the WAV bytes.
fn bounce(tmp: &Path, cache: &Path, project: &Path, sandbox: &str, mode: &str) -> Vec<u8> {
    let out = tmp.join(format!("{sandbox}-{mode}.wav"));
    let result = Command::new(render_exe())
        .arg(project)
        .arg("-o")
        .arg(&out)
        .args(["--allow-plugins", "--plugin-cache"])
        .arg(cache)
        .args(["--plugin-sandbox", sandbox, "--plugin-mode", mode])
        .output()
        .expect("atomik-render starts");
    assert!(
        result.status.success(),
        "{sandbox} {mode}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read(&out).expect("the WAV was written")
}

fn assert_same_bytes_sandboxed_or_not(tag: &str, project: &Path) {
    if !child_is_built() {
        return;
    }
    let tmp = std::env::temp_dir().join(format!(
        "atomik-render-sandbox-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let cache = tmp.join("plugins.json");
    test_cache().save(&cache).expect("cache written");
    for mode in ["offline", "realtime"] {
        let inside = bounce(&tmp, &cache, project, "off", mode);
        let outside = bounce(&tmp, &cache, project, "on", mode);
        assert!(!inside.is_empty());
        assert!(
            inside == outside,
            "{tag}, --plugin-mode {mode}: the sandboxed bounce differs from the in-process one"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn the_clap_demo_bounces_to_the_same_bytes_sandboxed_or_not() {
    assert_same_bytes_sandboxed_or_not("clap", &project("plugin-demo", "plugin-demo.atomik"));
}

#[test]
fn the_clap_and_vst3_demo_bounces_to_the_same_bytes_sandboxed_or_not() {
    assert_same_bytes_sandboxed_or_not(
        "vst3",
        &project("plugin-demo-vst3", "plugin-demo-vst3.atomik"),
    );
}
