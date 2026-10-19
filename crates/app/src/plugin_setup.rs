//! Start-up wiring of the plugin machinery: where the files live, where the
//! scanner program is, and the in-repo test plugins for `--stress-plugins`.
//!
//! The app never loads an unscanned plugin in its own process: the scanner is the
//! `atomik-plugin-scan` program next to the app's executable, run once per file with a hard
//! timeout. [`plugin_config`] reads what the previous runs left (the scan cache, the crash
//! journal) and hands [`crate::app_core::PluginConfig`] to the core.

use crate::app_core::{PluginConfig, PluginPaths};
use atomik_plugin_host::{
    CachedFile, ChildScanner, CrashJournal, FileStamp, PluginCache, PluginSettings,
    SCANNER_VERSION, ScanRunner, ScannerCommand,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The file name of the scanner program on this OS.
pub fn scanner_file_name() -> &'static str {
    if cfg!(windows) {
        "atomik-plugin-scan.exe"
    } else {
        "atomik-plugin-scan"
    }
}

/// The scanner program beside the executable `exe`, when it is there.
pub fn locate_scanner(exe: &Path) -> Option<PathBuf> {
    let path = exe.parent()?.join(scanner_file_name());
    path.is_file().then_some(path)
}

/// How to run the out-of-process scanner for the app executable `exe`: the `atomik-plugin-scan`
/// program beside it when it is there (preferred), else the executable itself with the hidden
/// `--plugin-scan` argument, which runs the same scanner code. Either way every file is loaded
/// in a child process that is killed at the timeout.
pub fn scanner_command(exe: &Path) -> ScannerCommand {
    match locate_scanner(exe) {
        Some(program) => ScannerCommand::new(program),
        None => {
            let mut command = ScannerCommand::new(exe);
            command.args.push(atomik_plugin_scan::SELF_SCAN_FLAG.into());
            command
        }
    }
}

/// The plugin config for a run: the cache and the crash journal read from `config_dir` (a missing
/// directory or file is an empty start), the recovery folder, and the real scanner when
/// `scanner` is given. Reading the journal removes it, so an unclean exit is reported once.
pub fn plugin_config(
    settings: PluginSettings,
    config_dir: Option<&Path>,
    scanner: Option<ScannerCommand>,
) -> PluginConfig {
    let paths = config_dir.map(PluginPaths::in_dir).unwrap_or_default();
    let cache = paths
        .cache
        .as_deref()
        .map(|p| PluginCache::load(p).0)
        .unwrap_or_default();
    let previous_crash = paths
        .journal
        .as_deref()
        .and_then(CrashJournal::take_previous);
    PluginConfig {
        settings,
        cache,
        paths,
        runner: scanner.map(|command| Arc::new(ChildScanner::new(command)) as Arc<dyn ScanRunner>),
        previous_crash,
    }
}

/// The in-repo CLAP test plugin library built next to the executable
/// (`cargo build -p atomik-test-clap`), installed as `<exe dir>/clap-plugins/atomik_test_clap.clap`
/// so a scanner finds it. `None` when the library has not been built.
pub fn install_test_plugin(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let library = dir.join(atomik_plugin_host::testing::cdylib_file_name(
        "atomik_test_clap",
    ));
    if !library.is_file() {
        return None;
    }
    let folder = dir.join("clap-plugins");
    std::fs::create_dir_all(&folder).ok()?;
    let target = folder.join(atomik_plugin_host::testing::clap_file_name(
        "atomik_test_clap",
    ));
    let stale = match (std::fs::metadata(&library), std::fs::metadata(&target)) {
        (Ok(a), Ok(b)) => a.modified().ok()? > b.modified().ok()?,
        _ => true,
    };
    if stale {
        // A copy that is loaded elsewhere cannot be replaced on Windows; the old copy is then
        // a copy of an earlier build of the same sources, which is fine.
        let _ = std::fs::copy(&library, &target);
    }
    target.is_file().then_some(target)
}

/// The in-repo VST3 test plugin library built next to the executable
/// (`cargo build -p atomik-test-vst3`), installed as a `.vst3` bundle with the layout of this OS,
/// `<exe dir>/vst3-plugins/atomik_test_vst3.vst3`, so a scanner finds it. `None` when the library
/// has not been built or this CPU has no bundle layout.
pub fn install_vst3_test_plugin(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let library = dir.join(atomik_plugin_host::testing::cdylib_file_name(
        "atomik_test_vst3",
    ));
    if !library.is_file() {
        return None;
    }
    let bundle = dir.join("vst3-plugins").join("atomik_test_vst3.vst3");
    let binary = bundle.join(atomik_plugin_host::testing::vst3_bundle_binary(
        atomik_plugin_host::paths::Os::current(),
        std::env::consts::ARCH,
        "atomik_test_vst3",
    )?);
    std::fs::create_dir_all(binary.parent()?).ok()?;
    if cfg!(target_os = "macos") {
        let plist = bundle.join("Contents/Info.plist");
        if !plist.is_file() {
            let text = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<plist version=\"1.0\"><dict>
<key>CFBundleExecutable</key><string>atomik_test_vst3</string>
<key>CFBundleIdentifier</key><string>invalid.atomik.test.atomik_test_vst3</string>
<key>CFBundlePackageType</key><string>BNDL</string>
</dict></plist>
";
            std::fs::write(&plist, text).ok()?;
        }
    }
    let stale = match (std::fs::metadata(&library), std::fs::metadata(&binary)) {
        (Ok(a), Ok(b)) => a.modified().ok()? > b.modified().ok()?,
        _ => true,
    };
    if stale {
        // A copy that is loaded elsewhere cannot be replaced on Windows; the old copy is then a
        // copy of an earlier build of the same sources, which is fine.
        let _ = std::fs::copy(&library, &binary);
    }
    binary.is_file().then_some(bundle)
}

/// Scans the in-repo test plugins of `--stress-plugins` beside the executable `exe` into the
/// config's cache, out of process through `runner`: the CLAP library is required, the VST3 one
/// (`cargo build -p atomik-test-vst3`) is used when it is there and the load is all CLAP when it
/// is not.
///
/// # Errors
/// A message when the CLAP library or the scanner is missing, or a scan fails.
pub fn scan_stress_plugins(
    config: &mut PluginConfig,
    exe: &Path,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let runner = config.runner.clone().ok_or_else(|| {
        "the plugin scanner program is not next to the app: run `cargo build -p atomik-plugin-scan`"
            .to_string()
    })?;
    let clap = install_test_plugin(exe).ok_or_else(|| {
        "the test plugin library is not next to the app: run `cargo build -p atomik-test-clap -p atomik-test-vst3`"
            .to_string()
    })?;
    add_scanned_file(config, &clap, runner.as_ref(), timeout)?;
    if let Some(vst3) = install_vst3_test_plugin(exe) {
        add_scanned_file(config, &vst3, runner.as_ref(), timeout)?;
    }
    Ok(())
}

/// Scans `file` with `runner` (out of process in the app) and puts the result into the config's
/// cache, unless the cache already lists it. The format comes from the file's extension. Used
/// for the test plugins of `--stress-plugins`.
///
/// # Errors
/// What the scanner said when it could not scan the file.
pub fn add_scanned_file(
    config: &mut PluginConfig,
    file: &Path,
    runner: &dyn ScanRunner,
    timeout: std::time::Duration,
) -> Result<(), String> {
    if config.cache.file(file).is_some() {
        return Ok(());
    }
    let cancel = AtomicBool::new(false);
    let format = atomik_render::plugins::format_of_file(file);
    let result = runner
        .scan(format, file, timeout, &cancel)
        .map_err(|f| format!("{}: {}", file.display(), f.reason))?;
    if let Some(error) = result.error {
        return Err(format!("{}: {error}", file.display()));
    }
    let stamp = FileStamp::of(file).ok_or_else(|| format!("{}: cannot read it", file.display()))?;
    config.cache.insert_file(CachedFile {
        path: file.to_path_buf(),
        format,
        stamp,
        scanner_version: SCANNER_VERSION,
        plugins: result.plugins,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_test_support::{PKG, cache_of, temp};
    use atomik_device_sdk::PluginFormat;
    use atomik_plugin_host::{FailureKind, ScanFailure, ScanResult};

    #[test]
    fn the_scanner_is_found_beside_the_executable() {
        let dir = temp("setup-scanner");
        let exe = dir.join(if cfg!(windows) {
            "atomik.exe"
        } else {
            "atomik"
        });
        assert_eq!(locate_scanner(&exe), None);
        let scanner = dir.join(scanner_file_name());
        std::fs::write(&scanner, b"x").unwrap();
        assert_eq!(locate_scanner(&exe), Some(scanner));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_scanner_command_prefers_the_program_and_falls_back_to_the_app_itself() {
        let dir = temp("setup-scanner-command");
        let exe = dir.join(if cfg!(windows) {
            "atomik.exe"
        } else {
            "atomik"
        });
        // No scanner beside the app: the app runs itself with the hidden argument.
        let fallback = scanner_command(&exe);
        assert_eq!(fallback.program, exe);
        assert_eq!(
            fallback.args,
            vec![std::ffi::OsString::from("--plugin-scan")]
        );
        // A scanner beside the app is preferred and needs no argument.
        let scanner = dir.join(scanner_file_name());
        std::fs::write(&scanner, b"x").unwrap();
        let preferred = scanner_command(&exe);
        assert_eq!(preferred.program, scanner);
        assert!(preferred.args.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_config_reads_the_cache_and_reports_an_unclean_exit_once() {
        let dir = temp("setup-config");
        // First run: nothing there. Plugins are off.
        let first = plugin_config(PluginSettings::default(), Some(&dir), None);
        assert!(!first.settings.enabled, "off by default");
        assert!(first.cache.files().next().is_none());
        assert!(first.previous_crash.is_none() && first.runner.is_none());
        assert_eq!(first.paths, PluginPaths::in_dir(&dir));
        // A cache and a journal from the previous run.
        cache_of(&[PKG]).save(&dir.join("plugins.json")).unwrap();
        std::fs::write(
            dir.join("plugins-in-use.json"),
            br#"{"version":1,"plugins":[{"format":"clap","id":"atomik.test.synth","path":"x.clap"}]}"#,
        )
        .unwrap();
        let second = plugin_config(
            PluginSettings::default(),
            Some(&dir),
            Some(ScannerCommand::new("scan")),
        );
        assert!(second.cache.files().next().is_some());
        assert_eq!(
            second.previous_crash.unwrap()[0].plugin.id,
            "atomik.test.synth"
        );
        assert!(second.runner.is_some());
        assert!(!dir.join("plugins-in-use.json").exists(), "reported once");
        let third = plugin_config(PluginSettings::default(), Some(&dir), None);
        assert!(third.previous_crash.is_none());
        let none = plugin_config(PluginSettings::default(), None, None);
        assert_eq!(none.paths, PluginPaths::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    struct Canned(
        Result<ScanResult, ScanFailure>,
        std::sync::Mutex<Vec<PluginFormat>>,
    );

    impl Canned {
        fn new(result: Result<ScanResult, ScanFailure>) -> Self {
            Self(result, std::sync::Mutex::new(Vec::new()))
        }
    }

    impl ScanRunner for Canned {
        fn scan(
            &self,
            format: PluginFormat,
            _path: &Path,
            _timeout: std::time::Duration,
            _cancel: &AtomicBool,
        ) -> Result<ScanResult, ScanFailure> {
            self.1.lock().unwrap().push(format);
            self.0.clone()
        }
    }

    #[test]
    fn a_scanned_test_plugin_file_goes_into_the_cache_once() {
        let dir = temp("setup-add");
        let file = dir.join("test.clap");
        std::fs::write(&file, b"lib").unwrap();
        let plugins = cache_of(&[PKG])
            .files()
            .flat_map(|f| f.plugins.clone())
            .collect::<Vec<_>>();
        let ok = Canned::new(Ok(ScanResult {
            path: file.clone(),
            format: PluginFormat::Clap,
            plugins,
            error: None,
        }));
        let mut config = plugin_config(PluginSettings::default(), None, None);
        add_scanned_file(&mut config, &file, &ok, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(config.cache.file(&file).unwrap().plugins.len(), 4);
        // A second call finds it listed: a failing scanner is not asked.
        let failing = Canned::new(Err(ScanFailure {
            kind: FailureKind::Crash,
            reason: "boom".into(),
        }));
        add_scanned_file(
            &mut config,
            &file,
            &failing,
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        let mut other = plugin_config(PluginSettings::default(), None, None);
        let e = add_scanned_file(
            &mut other,
            &file,
            &failing,
            std::time::Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(e.contains("boom"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_test_plugin_library_beside_the_executable_is_installed_as_a_clap_file() {
        let dir = temp("setup-install");
        let exe = dir.join(if cfg!(windows) {
            "atomik.exe"
        } else {
            "atomik"
        });
        assert_eq!(install_test_plugin(&exe), None, "not built");
        let lib = dir.join(atomik_plugin_host::testing::cdylib_file_name(
            "atomik_test_clap",
        ));
        std::fs::write(&lib, b"library").unwrap();
        let installed = install_test_plugin(&exe).unwrap();
        assert!(
            installed.ends_with(atomik_plugin_host::testing::clap_file_name(
                "atomik_test_clap"
            ))
        );
        assert!(installed.starts_with(dir.join("clap-plugins")));
        assert_eq!(std::fs::read(&installed).unwrap(), b"library");
        assert_eq!(install_test_plugin(&exe), Some(installed), "idempotent");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_vst3_file_is_scanned_as_vst3_and_cached_with_that_format() {
        let dir = temp("setup-add-vst3");
        let bundle = dir.join("test.vst3");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("inner"), b"lib").unwrap();
        let ok = Canned::new(Ok(ScanResult {
            path: bundle.clone(),
            format: PluginFormat::Vst3,
            plugins: Vec::new(),
            error: None,
        }));
        let mut config = plugin_config(PluginSettings::default(), None, None);
        add_scanned_file(&mut config, &bundle, &ok, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(*ok.1.lock().unwrap(), [PluginFormat::Vst3]);
        assert_eq!(
            config.cache.file(&bundle).unwrap().format,
            PluginFormat::Vst3
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_vst3_test_library_beside_the_executable_is_installed_as_a_bundle() {
        let dir = temp("setup-install-vst3");
        let exe = dir.join(if cfg!(windows) {
            "atomik.exe"
        } else {
            "atomik"
        });
        assert_eq!(install_vst3_test_plugin(&exe), None, "not built");
        let lib = dir.join(atomik_plugin_host::testing::cdylib_file_name(
            "atomik_test_vst3",
        ));
        std::fs::write(&lib, b"library").unwrap();
        let bundle = install_vst3_test_plugin(&exe).unwrap();
        assert!(bundle.ends_with("atomik_test_vst3.vst3"));
        assert!(bundle.starts_with(dir.join("vst3-plugins")));
        let binary = bundle.join(
            atomik_plugin_host::testing::vst3_bundle_binary(
                atomik_plugin_host::paths::Os::current(),
                std::env::consts::ARCH,
                "atomik_test_vst3",
            )
            .unwrap(),
        );
        assert_eq!(std::fs::read(&binary).unwrap(), b"library");
        assert_eq!(install_vst3_test_plugin(&exe), Some(bundle), "idempotent");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_stress_scan_needs_the_clap_library_and_takes_the_vst3_one_when_built() {
        let dir = temp("setup-stress");
        let exe = dir.join(if cfg!(windows) {
            "atomik.exe"
        } else {
            "atomik"
        });
        let result = |format| {
            Ok(ScanResult {
                path: PathBuf::new(),
                format,
                plugins: Vec::new(),
                error: None,
            })
        };
        let scanner = Arc::new(Canned::new(result(PluginFormat::Clap)));
        let mut config = plugin_config(PluginSettings::default(), None, None);
        let e =
            scan_stress_plugins(&mut config, &exe, std::time::Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("scanner program"), "{e}");
        config.runner = Some(scanner.clone());
        let e =
            scan_stress_plugins(&mut config, &exe, std::time::Duration::from_secs(5)).unwrap_err();
        assert!(
            e.contains("atomik-test-clap") && e.contains("atomik-test-vst3"),
            "{e}"
        );
        // Only the CLAP library is built: the load is all CLAP.
        std::fs::write(
            dir.join(atomik_plugin_host::testing::cdylib_file_name(
                "atomik_test_clap",
            )),
            b"clap",
        )
        .unwrap();
        scan_stress_plugins(&mut config, &exe, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(*scanner.1.lock().unwrap(), [PluginFormat::Clap]);
        // Both are built: both are scanned (a cached file is not scanned again).
        std::fs::write(
            dir.join(atomik_plugin_host::testing::cdylib_file_name(
                "atomik_test_vst3",
            )),
            b"vst3",
        )
        .unwrap();
        scan_stress_plugins(&mut config, &exe, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(
            *scanner.1.lock().unwrap(),
            [PluginFormat::Clap, PluginFormat::Vst3]
        );
        assert_eq!(config.cache.files().count(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
