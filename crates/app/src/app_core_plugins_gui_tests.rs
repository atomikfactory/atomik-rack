//! Wiring, second half: the editor
//! windows as the platform layer drives them (a fake handle and the test plugin's headless
//! editor), the crash journal and the recovery snapshots, and the scan job with a fake scanner.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::empty_doc;
use atomik_plugin_host::{
    CrashJournal, FailureKind, LoadStatus, NativeParent, PluginCache, PluginMessage, ScanFailure,
    ScanResult, ScanRunner, ScannedPlugin,
};
use atomik_ui::BannerKind;
use atomik_ui::plugin_data::{PluginBannerAction, PluginRescan, PluginSettingsAction};

// ------------------------------------------------------------------ editor windows

fn fake_parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0xA11C_E000)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0xA11C_E000)
    } else {
        NativeParent::X11(0x00A1_1CE0)
    }
}

fn window_info() -> PluginWindowInfo {
    PluginWindowInfo {
        parent: fake_parent(),
        scale: 1.5,
    }
}

fn gain_core() -> AppCore<OfflineSink> {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, GAIN, "echo");
    apply(
        &mut core,
        Command::Rename {
            key: key("echo"),
            name: "Echo".into(),
        },
    );
    core.frame();
    core
}

/// Opens the editor the way the platform layer does: the request, the window, the answer.
fn open_editor(core: &mut AppCore<OfflineSink>, device: &DeviceKey) -> Vec<PluginWindowRequest> {
    core.handle_action(UiAction::OpenPluginGui {
        device: device.clone(),
    });
    let first = core.take_plugin_window_requests();
    assert!(
        matches!(first.as_slice(), [PluginWindowRequest::Open { .. }]),
        "{first:?}"
    );
    core.plugin_window_created(device, window_info());
    core.take_plugin_window_requests()
}

#[test]
fn open_gui_asks_for_a_window_then_embeds_the_view_and_sizes_and_shows_it() {
    let mut core = gain_core();
    let dev = key("echo");
    assert!(
        core.view().plugin_status(&dev).has_editor,
        "OPEN GUI shows for a plugin with an editor"
    );
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    let asked = core.take_plugin_window_requests();
    assert_eq!(
        asked,
        vec![PluginWindowRequest::Open {
            device: dev.clone(),
            title: "Atomik Test Gain - Echo".to_string(),
            remembered: None,
        }]
    );
    assert!(core.plugin_gui_open(&dev), "opening");
    core.plugin_window_created(&dev, window_info());
    let after = core.take_plugin_window_requests();
    let [
        PluginWindowRequest::Resize { w, h, .. },
        PluginWindowRequest::Show { .. },
    ] = after.as_slice()
    else {
        panic!("size then show: {after:?}");
    };
    assert!(*w > 0 && *h > 0, "the plugin's own size: {w}x{h}");
    // A second OPEN GUI only shows the window again.
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Show { device: dev }]
    );
}

#[test]
fn the_window_and_the_plugin_resize_each_other() {
    let mut core = gain_core();
    let dev = key("echo");
    open_editor(&mut core, &dev);
    // The user drags the window edge: the plugin may adjust the size.
    core.plugin_window_resized(&dev, 640, 480);
    let reqs = core.take_plugin_window_requests();
    match reqs.as_slice() {
        [] => {}
        [PluginWindowRequest::Resize { w, h, .. }] => assert!((*w, *h) != (640, 480)),
        other => panic!("{other:?}"),
    }
    // The plugin asks for a bigger editor, from another thread: it arrives on the main thread
    // as a request for the platform, with the plugin's size.
    let id = core.plugin_instance(&dev).unwrap();
    let messenger = core
        .plugin_host()
        .manager_ref()
        .slot(id)
        .unwrap()
        .messenger();
    std::thread::spawn(move || {
        assert!(messenger.post(PluginMessage::GuiResize { w: 800, h: 600 }));
    })
    .join()
    .unwrap();
    assert!(
        core.take_plugin_window_requests().is_empty(),
        "nothing before the main thread polls"
    );
    core.frame();
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Resize {
            device: dev.clone(),
            w: 800,
            h: 600
        }]
    );
    // The plugin hides and shows its own window.
    post(&mut core, &dev, PluginMessage::GuiRequestHide);
    core.frame();
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Hide {
            device: dev.clone()
        }]
    );
    post(&mut core, &dev, PluginMessage::GuiRequestShow);
    core.frame();
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Show { device: dev }]
    );
}

#[test]
fn closing_the_window_captures_the_state_and_remembers_its_size() {
    let mut core = gain_core();
    let dev = key("echo");
    open_editor(&mut core, &dev);
    core.plugin_window_resized(&dev, 700, 500);
    let _ = core.take_plugin_window_requests();
    assert!(
        core.doc()
            .resolve(&dev)
            .unwrap()
            .plugin
            .as_ref()
            .unwrap()
            .state
            .is_none()
    );
    // The user closes the window: the platform has destroyed it, so no Close request.
    core.plugin_window_closed(&dev);
    assert!(!core.plugin_gui_open(&dev));
    assert!(core.take_plugin_window_requests().is_empty());
    let state = core
        .doc()
        .resolve(&dev)
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .state
        .clone();
    assert!(
        state.is_some_and(|s| !s.0.is_empty()),
        "the state was captured at the close"
    );
    // Reopening remembers the size of the last session.
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    let asked = core.take_plugin_window_requests();
    let [PluginWindowRequest::Open { remembered, .. }] = asked.as_slice() else {
        panic!("{asked:?}");
    };
    assert!(remembered.is_some());
    // The plugin closing its own editor asks the platform to destroy the window.
    core.plugin_window_created(&dev, window_info());
    let _ = core.take_plugin_window_requests();
    post(&mut core, &dev, PluginMessage::GuiClosed);
    core.frame();
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![PluginWindowRequest::Close {
            device: dev.clone()
        }]
    );
    assert!(!core.plugin_gui_open(&dev));
}

#[test]
fn removing_the_device_closes_its_editor_and_a_plugin_without_a_gui_has_no_button() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    add_plugin(&mut core, GAIN, "echo");
    core.frame();
    assert!(
        !core.view().plugin_status(&key("lead")).has_editor,
        "the synth has no editor"
    );
    core.handle_action(UiAction::OpenPluginGui {
        device: key("lead"),
    });
    assert!(core.last_rejection().is_some_and(|m| m.contains("editor")));
    assert!(core.take_plugin_window_requests().is_empty());
    open_editor(&mut core, &key("echo"));
    apply(&mut core, Command::RemoveDevice { key: key("echo") });
    let reqs = core.take_plugin_window_requests();
    assert!(
        reqs.contains(&PluginWindowRequest::Close {
            device: key("echo")
        }),
        "{reqs:?}"
    );
    assert!(!core.plugin_gui_open(&key("echo")));
}

#[test]
fn a_window_that_could_not_be_made_leaves_no_editor_open() {
    let mut core = gain_core();
    let dev = key("echo");
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    let _ = core.take_plugin_window_requests();
    core.plugin_window_failed(&dev, "no display");
    assert!(!core.plugin_gui_open(&dev));
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("no display"))
    );
}

#[test]
fn an_audio_restart_keeps_the_editor_open_but_a_new_project_closes_it() {
    let mut core = gain_core();
    let dev = key("echo");
    open_editor(&mut core, &dev);
    core.rebuild_engine();
    assert!(core.plugin_gui_open(&dev), "the instance lives on");
    assert!(core.take_plugin_window_requests().is_empty());
    core.open_doc(empty_doc());
    assert!(!core.plugin_gui_open(&dev));
    assert!(
        core.take_plugin_window_requests()
            .contains(&PluginWindowRequest::Close { device: dev })
    );
}

// ------------------------------------------------------------------ crash journal and recovery

fn config_in(dir: &Path, enabled: bool) -> PluginConfig {
    let mut c = config(enabled, &[PKG]);
    c.paths = PluginPaths::in_dir(dir);
    c
}

#[test]
fn the_crash_journal_lists_loaded_plugins_and_a_clean_exit_removes_it() {
    let dir = temp("journal");
    let journal = dir.join("plugins-in-use.json");
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config_in(&dir, true));
    assert!(!journal.exists(), "no plugin, no journal");
    add_plugin(&mut core, GAIN, "echo");
    let text = std::fs::read_to_string(&journal).expect("written at the first instance");
    assert!(text.contains("atomik.test.gain"), "{text}");
    apply(&mut core, Command::RemoveDevice { key: key("echo") });
    settle(&mut core);
    assert!(
        !journal.exists(),
        "the last instance went: the journal is removed"
    );
    add_plugin(&mut core, SYNTH, "lead");
    assert!(journal.exists());
    core.shutdown_plugins();
    assert!(!journal.exists(), "a clean exit removes it");
    let _ = std::fs::remove_dir_all(dir);
}

fn clap_plugin(id: &str) -> atomik_device_sdk::PluginRef {
    atomik_device_sdk::PluginRef {
        format: atomik_device_sdk::PluginFormat::Clap,
        id: id.to_string(),
    }
}

#[test]
fn an_unclean_exit_offers_disable_and_the_recovery_snapshot() {
    let dir = temp("unclean");
    {
        let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
        core.configure_plugins(config_in(&dir, true));
        add_plugin(&mut core, SYNTH, "lead");
        set_param(&mut core, "lead", "p2", 0.4);
        core.plugin_tick(0.0);
        core.wait_recovery_writes();
        // The process "crashes": the journal and the snapshot stay.
        std::mem::forget(core);
    }
    let previous = CrashJournal::take_previous(&dir.join("plugins-in-use.json"))
        .expect("the journal of the crashed run");
    assert_eq!(previous[0].plugin.id, "atomik.test.synth");
    let mut cfg = config_in(&dir, true);
    cfg.previous_crash = Some(previous);
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(cfg);
    match core.plugin_banner() {
        Some(BannerKind::UncleanExit {
            plugins,
            has_recovery,
        }) => {
            assert_eq!(plugins, &vec!["Atomik Test Synth".to_string()]);
            assert!(*has_recovery);
        }
        other => panic!("{other:?}"),
    }
    // OPEN RECOVERY opens the newest snapshot as an unsaved project.
    core.handle_action(UiAction::PluginBanner(PluginBannerAction::OpenRecovery));
    core.wait_background();
    assert!(
        core.doc().resolve(&key("lead")).is_some(),
        "the device is in the snapshot"
    );
    assert_eq!(
        core.doc()
            .resolve(&key("lead"))
            .unwrap()
            .params
            .get("p2")
            .copied(),
        Some(0.4)
    );
    assert!(
        core.is_dirty() && core.file().is_none(),
        "not the user's file"
    );
    // DISABLE blacklists the plugin file and takes its plugins out of the catalog.
    core.handle_action(UiAction::PluginBanner(PluginBannerAction::DisableCrashed));
    assert!(core.plugin_banner().is_none());
    assert!(
        core.plugin_catalog()
            .entry(&clap_plugin("atomik.test.synth"))
            .is_none()
    );
    assert!(
        !core.plugin_catalog().failed().is_empty(),
        "the file is on the blacklist"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn recovery_snapshots_are_written_every_minute_rotate_to_three_and_never_touch_the_users_file() {
    let dir = temp("recovery");
    let project = dir.join("song.atomik");
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config_in(&dir, true));
    // Nothing loaded: no snapshot, however long it runs.
    core.plugin_tick(0.0);
    core.plugin_tick(500.0);
    core.wait_recovery_writes();
    assert!(std::fs::read_dir(dir.join("recovery")).is_ok_and(|d| {
        d.filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().ends_with(".recover.atomik"))
    }));
    add_plugin(&mut core, SYNTH, "lead");
    core.save_as(&project).unwrap();
    let user_file = std::fs::read(&project).unwrap();
    let count = || {
        std::fs::read_dir(dir.join("recovery"))
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".recover.atomik"))
                    .count()
            })
            .unwrap_or(0)
    };
    core.plugin_tick(1000.0);
    core.wait_recovery_writes();
    assert_eq!(count(), 1, "the first tick with a plugin loaded writes one");
    core.plugin_tick(1030.0);
    core.wait_recovery_writes();
    assert_eq!(count(), 1, "not before a minute has passed");
    for (i, now) in [1061.0, 1122.0, 1183.0, 1244.0].into_iter().enumerate() {
        core.plugin_tick(now);
        core.wait_recovery_writes();
        assert_eq!(count(), (i + 2).min(3), "kept: the last three (tick {now})");
    }
    // A state capture asks for a snapshot at the next tick, before the minute is up.
    post(&mut core, &key("lead"), PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(1245.0);
    core.plugin_tick(1247.0);
    core.wait_recovery_writes();
    let newest = std::fs::read_dir(dir.join("recovery"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .max()
        .unwrap();
    assert!(
        std::fs::read_to_string(newest)
            .unwrap()
            .contains("\"state\":"),
        "the snapshot holds the plugin state"
    );
    assert_eq!(
        std::fs::read(&project).unwrap(),
        user_file,
        "the user's file is untouched"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------------ scanning

struct FakeScanner {
    dir: PathBuf,
    plugin: ScannedPlugin,
}

impl ScanRunner for FakeScanner {
    fn scan(
        &self,
        format: atomik_device_sdk::PluginFormat,
        path: &Path,
        _timeout: std::time::Duration,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<ScanResult, ScanFailure> {
        let fail = |kind, reason: &str| ScanFailure {
            kind,
            reason: reason.to_string(),
        };
        if !path.starts_with(&self.dir) {
            return Err(fail(FailureKind::Error, "not a test file"));
        }
        match path.file_name().and_then(|n| n.to_str()) {
            Some("good.clap") => Ok(ScanResult {
                path: path.to_path_buf(),
                format,
                plugins: vec![self.plugin.clone()],
                error: None,
            }),
            _ => Err(fail(FailureKind::Crash, "segfault")),
        }
    }
}

fn scan_core(dir: &Path) -> AppCore<OfflineSink> {
    let folder = dir.join("clap");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("good.clap"), b"good").unwrap();
    std::fs::write(folder.join("bad.clap"), b"bad").unwrap();
    let cache = cache_of(&[PKG]);
    let plugin = cache
        .files()
        .flat_map(|f| f.plugins.clone())
        .find(|p| p.plugin.id == "atomik.test.synth")
        .unwrap();
    let mut cfg = config_in(dir, true);
    cfg.cache = PluginCache::default();
    cfg.settings.extra_paths = vec![folder.clone()];
    cfg.runner = Some(std::sync::Arc::new(FakeScanner {
        dir: folder,
        plugin,
    }));
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(cfg);
    core.set_plugin_scan_env(Arc::new(|_: &str| None));
    core
}

#[test]
fn a_scan_job_lists_the_found_plugins_blacklists_the_failed_files_and_saves_the_cache() {
    let dir = temp("scan");
    let mut core = scan_core(&dir);
    assert!(core.plugin_catalog().entries().is_empty());
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Incremental,
    )));
    assert!(core.plugin_scan_running() || core.plugin_scan_note().is_some());
    core.wait_scan();
    assert!(!core.plugin_scan_running());
    let ids: Vec<String> = core
        .plugin_catalog()
        .entries()
        .iter()
        .map(|e| e.plugin.type_id())
        .collect();
    assert!(ids.contains(&SYNTH.to_string()), "{ids:?}");
    assert!(
        core.registry().is_plugin(SYNTH),
        "registered from the cache, nothing loaded"
    );
    let data = core.plugin_settings_data();
    assert!(
        data.plugins
            .iter()
            .any(|r| r.plugin.id == "atomik.test.synth")
    );
    assert!(
        data.failed
            .iter()
            .any(|f| f.path.ends_with("bad.clap") && f.reason.contains("segfault")),
        "{:?}",
        data.failed
    );
    assert!(data.scan.is_none());
    assert!(
        data.log.iter().any(|l| l.starts_with("scan:")),
        "{:?}",
        data.log
    );
    // The cache went to disk, atomically, and loads back.
    let (cache, status) = PluginCache::load(&dir.join("plugins.json"));
    assert!(matches!(status, LoadStatus::Loaded), "{status:?}");
    assert!(cache.files().count() >= 1 && cache.blacklist().count() >= 1);
    // RETRY takes the file off the blacklist and scans it again (it fails again here).
    let bad = data
        .failed
        .iter()
        .find(|f| f.path.ends_with("bad.clap"))
        .unwrap()
        .path
        .clone();
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::RetryFile(
        bad,
    )));
    core.wait_scan();
    assert!(
        core.plugin_catalog()
            .failed()
            .iter()
            .any(|f| f.path.ends_with("bad.clap"))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Remembers what it was asked to scan and calls every file good.
struct Recording {
    seen: std::sync::Mutex<Vec<(atomik_device_sdk::PluginFormat, String)>>,
    plugin: ScannedPlugin,
}

impl ScanRunner for Recording {
    fn scan(
        &self,
        format: atomik_device_sdk::PluginFormat,
        path: &Path,
        _timeout: std::time::Duration,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<ScanResult, ScanFailure> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        self.seen.lock().unwrap().push((format, name.clone()));
        if name.starts_with("bad") {
            return Err(ScanFailure {
                kind: FailureKind::Crash,
                reason: "the scanner exited with code 3".to_string(),
            });
        }
        // Each file gets its own plugin id so the catalog lists all of them.
        let mut plugin = self.plugin.clone();
        plugin.plugin.id = format!("test.{name}");
        plugin.plugin.format = format;
        Ok(ScanResult {
            path: path.to_path_buf(),
            format,
            plugins: vec![plugin],
            error: None,
        })
    }
}

fn recording_core(dir: &Path, folders: Vec<PathBuf>) -> (AppCore<OfflineSink>, Arc<Recording>) {
    let plugin = cache_of(&[PKG])
        .files()
        .flat_map(|f| f.plugins.clone())
        .next()
        .unwrap();
    let runner = Arc::new(Recording {
        seen: std::sync::Mutex::new(Vec::new()),
        plugin,
    });
    let mut cfg = config_in(dir, true);
    cfg.cache = PluginCache::default();
    cfg.settings.extra_paths = folders;
    cfg.runner = Some(runner.clone());
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(cfg);
    // Only the folders the settings list: not this machine's real plugin folders.
    core.set_plugin_scan_env(Arc::new(|_: &str| None));
    (core, runner)
}

fn scan_all(core: &mut AppCore<OfflineSink>) {
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Full,
    )));
    core.wait_scan();
}

#[test]
fn a_scan_searches_user_folders_for_clap_and_vst3_files_recursively() {
    use atomik_device_sdk::PluginFormat::{Clap, Vst3};
    let dir = temp("scan-formats");
    let folder = dir.join("plugins");
    std::fs::create_dir_all(folder.join("Vendor/Suite")).unwrap();
    std::fs::write(folder.join("a.clap"), b"x").unwrap();
    std::fs::write(folder.join("single.vst3"), b"x").unwrap();
    // A bundle (a folder named *.vst3) is one plugin file; the module inside is not another.
    std::fs::create_dir_all(folder.join("Vendor/Suite/Bundle.vst3/Contents/x86_64-win")).unwrap();
    std::fs::write(
        folder.join("Vendor/Suite/Bundle.vst3/Contents/x86_64-win/Bundle.vst3"),
        b"x",
    )
    .unwrap();
    std::fs::write(folder.join("Vendor/old.dll"), b"x").unwrap();
    let (mut core, runner) = recording_core(&dir, vec![folder]);
    scan_all(&mut core);
    let mut seen = runner.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (Clap, "a.clap".to_string()),
            (Vst3, "Bundle.vst3".to_string()),
            (Vst3, "single.vst3".to_string()),
        ]
    );
    // The found list shows every scanned plugin with its format, and it refreshed by itself.
    let data = core.plugin_settings_data();
    let rows: Vec<(String, atomik_device_sdk::PluginFormat)> = data
        .plugins
        .iter()
        .map(|r| (r.plugin.id.clone(), r.plugin.format))
        .collect();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows.iter().any(|(_, f)| *f == Vst3) && rows.iter().any(|(_, f)| *f == Clap));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_scan_logs_each_failed_file_with_its_reason_then_a_summary_the_view_shows() {
    let dir = temp("scan-log");
    let folder = dir.join("plugins");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("good.vst3"), b"x").unwrap();
    std::fs::write(folder.join("bad.vst3"), b"x").unwrap();
    let (mut core, _) = recording_core(&dir, vec![folder]);
    scan_all(&mut core);
    let data = core.plugin_settings_data();
    let failure = data
        .log
        .iter()
        .position(|l| l.contains("bad.vst3") && l.contains("the scanner exited with code 3"))
        .unwrap_or_else(|| panic!("{:?}", data.log));
    let summary = data
        .log
        .iter()
        .position(|l| l.starts_with("scan: 2 scanned") && l.contains("1 ok, 1 failed"))
        .unwrap_or_else(|| panic!("{:?}", data.log));
    assert!(failure < summary, "the summary comes last");
    assert_eq!(data.last_scan.as_deref(), Some(data.log[summary].as_str()));
    assert!(data.scan.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_folder_of_only_vst2_plugins_gets_a_one_line_hint() {
    let dir = temp("scan-vst2");
    let vst2 = dir.join("VSTPlugins");
    std::fs::create_dir_all(&vst2).unwrap();
    std::fs::write(vst2.join("synth.dll"), b"x").unwrap();
    let (mut core, runner) = recording_core(&dir, vec![vst2.clone()]);
    scan_all(&mut core);
    assert!(runner.seen.lock().unwrap().is_empty());
    let log = core.plugin_settings_data().log;
    let hints: Vec<&String> = log.iter().filter(|l| l.contains("VST2")).collect();
    assert_eq!(hints.len(), 1, "{log:?}");
    assert!(
        hints[0].contains("not supported") && hints[0].contains("VSTPlugins"),
        "{}",
        hints[0]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_scan_without_a_scanner_or_without_plugins_enabled_says_so_persistently() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[]));
    assert!(!core.plugin_settings_data().scanner_available);
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Full,
    )));
    let data = core.plugin_settings_data();
    assert!(
        data.last_scan
            .as_deref()
            .is_some_and(|m| m.contains("scanner")),
        "{data:?}"
    );
    assert!(data.log.iter().any(|l| l.contains("scanner")));
    // Plugins off: the same, naming the switch.
    core.configure_plugins(config(false, &[]));
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Full,
    )));
    let data = core.plugin_settings_data();
    assert!(!data.enabled);
    assert!(
        data.last_scan
            .as_deref()
            .is_some_and(|m| m.contains("enable")),
        "{data:?}"
    );
}

#[test]
fn scanning_needs_plugins_enabled_and_a_scanner() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(false, &[]));
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Full,
    )));
    assert!(core.last_rejection().is_some_and(|m| m.contains("enable")));
    assert!(!core.plugin_scan_running());
    core.configure_plugins(config(true, &[]));
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::Scan(
        PluginRescan::Full,
    )));
    assert!(core.last_rejection().is_some_and(|m| m.contains("scanner")));
    // Folders and the timeout are settings changes the platform saves.
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::AddFolderDialog,
    ));
    assert!(core.take_plugin_folder_request());
    assert!(!core.take_plugin_folder_request());
    core.add_plugin_folder(PathBuf::from("D:/Plugins/CLAP"));
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::SetTimeout(
        120,
    )));
    let s = core.take_plugin_settings_change().unwrap();
    assert_eq!(s.timeout_secs, 120);
    assert_eq!(s.extra_paths, vec![PathBuf::from("D:/Plugins/CLAP")]);
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::RemoveFolder("D:/Plugins/CLAP".into()),
    ));
    assert!(
        core.take_plugin_settings_change()
            .unwrap()
            .extra_paths
            .is_empty()
    );
}

// ------------------------------------------------------------------ the parameter pane and the feeds

#[test]
fn plugin_devices_publish_their_meter_values_and_the_pane_gets_param_texts() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, GAIN, "echo");
    settle(&mut core);
    // The device's published values reach the views: state (0 running), latency, dropped.
    let mut values = [0.0f32; 8];
    let (n, _) = core
        .device_meter_values(&key("echo"), &mut values)
        .expect("a plugin device publishes meter values");
    assert_eq!(n, atomik_plugin_host::METER_VALUES);
    assert_eq!(values[2], 0.0, "running");
    // The pane lists the params of the live plugin and gets the plugin's own display texts.
    core.handle_action(UiAction::OpenPluginParams {
        device: key("echo"),
    });
    assert_eq!(core.plugin_params_device(), Some(&key("echo")));
    assert_eq!(
        core.plugin_params().total_count(),
        4,
        "gain, mute, latency, duck"
    );
    core.frame();
    assert!(
        core.plugin_params_mut().take_text_requests().is_empty(),
        "the requests were answered by the frame"
    );
    // The pane closes with its action and when the device goes away.
    core.handle_action(UiAction::ClosePluginParams);
    assert!(core.plugin_params_device().is_none());
    core.open_plugin_params(&key("echo"));
    apply(&mut core, Command::RemoveDevice { key: key("echo") });
    assert!(core.plugin_params_device().is_none());
}

#[test]
fn automate_adds_a_lane_for_the_param_on_the_track_of_the_selected_clip() {
    use atomik_model::{Clip, ClipContent, ClipId, Note, SongCommand};
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    apply(
        &mut core,
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
    );
    // Nothing selected: a message, no lane.
    core.handle_action(UiAction::AutomatePluginParam {
        device: key("lead"),
        param: 2,
    });
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("select a clip"))
    );
    apply(
        &mut core,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: atomik_core::PPQ * 4,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, atomik_core::PPQ, 60, 0.8)]),
            },
        }),
    );
    let clip = core.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    core.arrangement_mut().set_selected_clips(&[clip]);
    core.handle_action(UiAction::AutomatePluginParam {
        device: key("lead"),
        param: 2,
    });
    let lanes = &core.doc().song.track(&key("lead")).unwrap().lanes;
    assert_eq!(lanes.len(), 2, "the notes lane and the new automation lane");
    assert!(matches!(
        &lanes[1].kind,
        atomik_model::LaneKind::Automation(p) if p == "p2"
    ));
    assert!(core.undo(), "one undo step");
    assert_eq!(core.doc().song.track(&key("lead")).unwrap().lanes.len(), 1);
}

#[test]
fn a_plugin_that_fails_at_run_time_shows_failed_with_the_reason_and_reload_tries_again() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG, "atomik-test-clap-variants"]));
    add_plugin(&mut core, "clap.atomik.test.error", "bad");
    add_plugin(&mut core, SYNTH, "lead");
    settle(&mut core);
    let reason = core.plugin_failure(&key("bad")).map(str::to_string);
    assert!(reason.is_some(), "the device reported its failure");
    assert!(core.view().plugin_status(&key("bad")).failed.is_some());
    assert!(
        core.view().plugin_status(&key("bad")).missing.is_none(),
        "FAILED, not MISSING"
    );
    assert!(
        core.plugin_failure(&key("lead")).is_none(),
        "the others are fine"
    );
    assert!(
        core.plugin_log().iter().any(|l| l.contains("failed")),
        "{:?}",
        core.plugin_log()
    );
    // RELOAD resets the device; this plugin fails again at its first block.
    core.handle_action(UiAction::ReloadPlugin { device: key("bad") });
    assert!(
        core.plugin_failure(&key("bad")).is_none(),
        "cleared by RELOAD"
    );
    settle(&mut core);
    assert!(core.plugin_failure(&key("bad")).is_some());
}
