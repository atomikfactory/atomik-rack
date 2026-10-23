#![allow(clippy::disallowed_methods)] // Justification: tests wait for real child processes.
//! The plugin scan as the user runs it, end to end through the Settings path (the bug: SCAN on a
//! folder of working VST3 plugins found nothing, and the scanner program had to be built
//! separately): the app scans CLAP and VST3 files, and when no `atomik-plugin-scan` program is
//! beside the executable it runs its own executable (`atomik --plugin-scan <format> <path>`) as
//! the out-of-process scanner. Real child processes, the real in-repo test plugins.

use atomik_app::AppCore;
use atomik_app::app_core::PluginConfig;
use atomik_app::plugin_setup::{plugin_config, scanner_command, scanner_file_name};
use atomik_app::session::{LiveSession, SystemBackend};
use atomik_app::settings::AudioSettings;
use atomik_device_sdk::PluginFormat;
use atomik_plugin_host::PluginSettings;
use atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin, vst3_test_plugin_file};
use atomik_ui::UiAction;
use atomik_ui::plugin_data::{PluginRescan, PluginSettingsAction};
use std::path::{Path, PathBuf};
use std::process::Command;

const ATOMIK: &str = env!("CARGO_BIN_EXE_atomik");

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-scan-fallback-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            copy_tree(&e.path(), &to.join(e.file_name()));
        }
    } else {
        std::fs::copy(from, to).unwrap();
    }
}

/// A copy of the app executable alone in a fresh folder: no scanner program beside it.
fn lonely_exe(dir: &Path) -> PathBuf {
    let folder = dir.join("app");
    std::fs::create_dir_all(&folder).unwrap();
    let exe = folder.join(Path::new(ATOMIK).file_name().unwrap());
    std::fs::copy(ATOMIK, &exe).unwrap();
    assert!(!folder.join(scanner_file_name()).exists());
    exe
}

fn core_scanning(exe: &Path, folders: Vec<PathBuf>) -> AppCore<LiveSession<SystemBackend>> {
    let mut settings = PluginSettings::default();
    settings.enable();
    settings.extra_paths = folders;
    settings.set_timeout_secs(60);
    let config: PluginConfig = plugin_config(settings, None, Some(scanner_command(exe)));
    let session = LiveSession::new(SystemBackend, AudioSettings::default(), false);
    let mut core = AppCore::new(session);
    core.configure_plugins(config);
    // Only the folders given, not this machine's real plugin folders.
    core.set_plugin_scan_env(std::sync::Arc::new(|_: &str| None));
    core
}

fn scan(core: &mut AppCore<LiveSession<SystemBackend>>) {
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Full,
    )));
    core.wait_scan();
}

#[test]
fn the_hidden_scan_argument_prints_the_result_line_and_exits_zero_or_two() {
    let dir = temp("cli");
    let clap = dir.join("atomik_test_clap.clap");
    std::fs::copy(clap_test_plugin("atomik-test-clap").unwrap(), &clap).unwrap();
    let out = Command::new(ATOMIK)
        .args(["--plugin-scan", "clap"])
        .arg(&clap)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .expect("a JSON line");
    let result = atomik_plugin_host::ScanResult::from_json(line).unwrap();
    assert_eq!(result.format, PluginFormat::Clap);
    assert!(result.plugins.len() >= 3, "{:?}", result.error);
    // A missing file is still a clean result with an error.
    let missing = Command::new(ATOMIK)
        .args(["--plugin-scan", "vst3", "no-such-file.vst3"])
        .output()
        .unwrap();
    assert!(missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stdout).contains("\"error\""));
    // Bad arguments: exit code 2, nothing on stdout.
    let bad = Command::new(ATOMIK)
        .args(["--plugin-scan", "vst2", "x"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(bad.stdout.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn scanning_a_folder_finds_clap_and_vst3_plugins_with_no_scanner_program_beside_the_app() {
    let dir = temp("both");
    let exe = lonely_exe(&dir);
    let folder = dir.join("plugins");
    // The layout of a real plugin folder: a CLAP file, a VST3 bundle inside a vendor folder, and
    // a VST2 dll that is ignored.
    std::fs::create_dir_all(folder.join("Vendor")).unwrap();
    std::fs::copy(
        clap_test_plugin("atomik-test-clap").unwrap(),
        folder.join("atomik_test_clap.clap"),
    )
    .unwrap();
    copy_tree(
        &vst3_test_plugin("atomik-test-vst3").unwrap(),
        &folder.join("Vendor/atomik_test_vst3.vst3"),
    );
    std::fs::write(folder.join("old_vst2.dll"), b"not a vst3").unwrap();
    let mut core = core_scanning(&exe, vec![folder]);
    assert!(
        core.plugin_settings_data().scanner_available,
        "the app can scan with itself"
    );
    scan(&mut core);
    let data = core.plugin_settings_data();
    let formats: Vec<PluginFormat> = data.plugins.iter().map(|r| r.plugin.format).collect();
    assert!(formats.contains(&PluginFormat::Clap), "{data:?}");
    assert!(formats.contains(&PluginFormat::Vst3), "{:?}", data.log);
    assert!(
        data.plugins
            .iter()
            .all(|r| !r.name.is_empty() && !r.vendor.is_empty())
    );
    assert!(data.failed.is_empty(), "{:?}", data.failed);
    let summary = core.plugin_scan_note().unwrap_or_default();
    assert!(
        summary.contains("2 scanned") && summary.contains("2 ok"),
        "{summary}"
    );
    assert!(data.last_scan.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

/// A VST3 module copied as a plain `single.vst3` file in a fresh plugin folder.
fn single_file_vst3_folder(dir: &Path) -> PathBuf {
    let folder = dir.join("plugins");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(
        vst3_test_plugin_file("atomik-test-vst3").unwrap(),
        folder.join("single.vst3"),
    )
    .unwrap();
    folder
}

// Windows and Linux accept a single-file VST3; on macOS a VST3 is always a bundle folder.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_single_file_vst3_is_found_too() {
    let dir = temp("flat");
    let exe = lonely_exe(&dir);
    let folder = single_file_vst3_folder(&dir);
    let mut core = core_scanning(&exe, vec![folder]);
    scan(&mut core);
    let data = core.plugin_settings_data();
    assert!(
        data.plugins
            .iter()
            .any(|r| r.plugin.format == PluginFormat::Vst3),
        "{:?} {:?}",
        data.log,
        data.failed
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Manual smoke test on real plugin folders (read-only: every plugin is loaded in a child
/// process, nothing is written to the folders): `ATOMIK_SCAN_SMOKE=<folder>[;<folder>...] cargo
/// test -p atomik-app --test plugin_scan_fallback -- --ignored --nocapture`.
#[test]
#[ignore = "scans the folders in ATOMIK_SCAN_SMOKE (real plugins), run by hand"]
fn smoke_scan_of_real_folders() {
    let folders: Vec<PathBuf> = std::env::var("ATOMIK_SCAN_SMOKE")
        .expect("ATOMIK_SCAN_SMOKE names the folders")
        .split(';')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    let dir = temp("smoke");
    let exe = lonely_exe(&dir);
    let mut core = core_scanning(&exe, folders);
    let started = std::time::Instant::now();
    scan(&mut core);
    let data = core.plugin_settings_data();
    println!("scan took {:?}", started.elapsed());
    for l in &data.log {
        println!("LOG {l}");
    }
    for r in &data.plugins {
        println!(
            "PLUGIN {:?} | {} | {} | {} | {:?}",
            r.plugin.format, r.name, r.vendor, r.version, r.status
        );
    }
    for f in &data.failed {
        println!("FAILED {} | {}", f.path, f.reason);
    }
    println!(
        "{} plugins, {} failed files",
        data.plugins.len(),
        data.failed.len()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_broken_file_is_a_failure_with_a_reason_and_the_rest_still_scan() {
    let dir = temp("broken");
    let exe = lonely_exe(&dir);
    let folder = dir.join("plugins");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(
        clap_test_plugin("atomik-test-clap").unwrap(),
        folder.join("good.clap"),
    )
    .unwrap();
    std::fs::write(folder.join("junk.vst3"), b"this is not a module").unwrap();
    let mut core = core_scanning(&exe, vec![folder]);
    scan(&mut core);
    let data = core.plugin_settings_data();
    assert!(!data.plugins.is_empty());
    assert!(
        data.failed.iter().any(|f| f.path.ends_with("junk.vst3")),
        "{:?}",
        data.failed
    );
    assert!(
        data.log
            .iter()
            .any(|l| l.contains("junk.vst3") && l.contains("failed")),
        "the failure and its reason are in the log: {:?}",
        data.log
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(target_os = "macos")]
#[test]
fn a_single_file_vst3_is_refused_on_macos_without_crashing_the_scanner() {
    let dir = temp("flat-mac");
    let exe = lonely_exe(&dir);
    let folder = single_file_vst3_folder(&dir);
    let mut core = core_scanning(&exe, vec![folder]);
    scan(&mut core);
    let data = core.plugin_settings_data();
    assert!(
        !data
            .plugins
            .iter()
            .any(|r| r.plugin.format == PluginFormat::Vst3),
        "{:?}",
        data.plugins
    );
    assert!(
        data.failed.iter().all(|f| !f.reason.contains("signal")),
        "the scanner must refuse the file, not crash: {:?}",
        data.failed
    );
    let _ = std::fs::remove_dir_all(dir);
}
